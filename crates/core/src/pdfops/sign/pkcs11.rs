//! Signing keys held by a PKCS#11 token: an HSM, a smart card, SoftHSM. The
//! private key never leaves the token; this service asks it for signatures.
//!
//! The module (the vendor's `.so`) is loaded once per path and initialised
//! for use from several threads. Each key gets its own logged-in session,
//! used by one signature at a time.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use cryptoki::context::{CInitializeArgs, CInitializeFlags, Pkcs11};
use cryptoki::error::{Error as CkError, RvError};
use cryptoki::mechanism::Mechanism;
use cryptoki::object::{Attribute, ObjectClass, ObjectHandle};
use cryptoki::session::{Session, UserType};
use cryptoki::slot::Slot;
use cryptoki::types::AuthPin;

/// Where the token is and how to log in to it, from `RUSTICDOC_PKCS11_*`.
#[derive(Clone, Default, PartialEq)]
pub struct Pkcs11Settings {
    /// The PKCS#11 module, such as `/usr/lib/softhsm/libsofthsm2.so`.
    pub module: Option<PathBuf>,
    /// The token's label. The first token with a key of the right label when
    /// neither this nor `slot` is set.
    pub token: Option<String>,
    /// The slot id.
    pub slot: Option<u64>,
    /// The user PIN, from the environment.
    pub pin: Option<String>,
    /// A file holding the user PIN, which wins over `pin`.
    pub pin_file: Option<PathBuf>,
}

impl std::fmt::Debug for Pkcs11Settings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pkcs11Settings")
            .field("module", &self.module)
            .field("token", &self.token)
            .field("slot", &self.slot)
            .field("pin_file", &self.pin_file)
            .finish_non_exhaustive()
    }
}

/// A key on a token, as `RUSTICDOC_SIGNING_KEY=pkcs11:...` names it:
/// `pkcs11:<label>`, or RFC 7512 style attributes `object=`, `token=` and
/// `slot-id=` separated by `;`.
#[derive(Debug, Clone, PartialEq)]
pub struct TokenKeySpec {
    pub label: String,
    pub token: Option<String>,
    pub slot: Option<u64>,
}

impl TokenKeySpec {
    /// Parses what follows `pkcs11:`.
    pub fn parse(spec: &str) -> Result<Self, String> {
        if !spec.contains('=') {
            if spec.trim().is_empty() {
                return Err("'pkcs11:' needs the key's label after it".into());
            }
            return Ok(Self {
                label: spec.trim().to_string(),
                token: None,
                slot: None,
            });
        }
        let mut parsed = Self {
            label: String::new(),
            token: None,
            slot: None,
        };
        for part in spec.split(';').map(str::trim).filter(|p| !p.is_empty()) {
            let (key, value) = part
                .split_once('=')
                .ok_or_else(|| format!("'{part}' in the pkcs11: key is not name=value"))?;
            let value = percent_decode(value);
            match key {
                "object" => parsed.label = value,
                "token" => parsed.token = Some(value),
                "slot-id" => {
                    parsed.slot = Some(
                        value
                            .parse()
                            .map_err(|_| format!("slot-id '{value}' is not a number"))?,
                    )
                }
                // Attributes that change nothing here.
                "type"
                | "id"
                | "manufacturer"
                | "model"
                | "serial"
                | "library-description"
                | "library-manufacturer"
                | "library-version" => {}
                other => {
                    return Err(format!(
                        "the pkcs11: key has an unknown attribute '{other}'"
                    ));
                }
            }
        }
        if parsed.label.is_empty() {
            return Err("the pkcs11: key needs object=<label>".into());
        }
        Ok(parsed)
    }
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Some(b) = std::str::from_utf8(&bytes[i + 1..i + 3])
                .ok()
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(b);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The loaded modules, by path: a module is initialised once per process.
fn context(module: &Path) -> Result<Pkcs11, String> {
    static CONTEXTS: OnceLock<Mutex<HashMap<PathBuf, Pkcs11>>> = OnceLock::new();
    let mut contexts = CONTEXTS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(context) = contexts.get(module) {
        return Ok(context.clone());
    }
    let context = Pkcs11::new(module)
        .map_err(|e| format!("cannot load the PKCS#11 module {}: {e}", module.display()))?;
    match context.initialize(CInitializeArgs::new(CInitializeFlags::OS_LOCKING_OK)) {
        Ok(()) | Err(CkError::Pkcs11(RvError::CryptokiAlreadyInitialized, _)) => {}
        Err(e) => {
            return Err(format!(
                "cannot initialise the PKCS#11 module {}: {e}",
                module.display()
            ));
        }
    }
    contexts.insert(module.to_path_buf(), context.clone());
    Ok(context)
}

/// A private key on a token, with a logged-in session to use it.
pub(super) struct TokenKey {
    session: Mutex<Session>,
    key: ObjectHandle,
    /// `label` on token `token`, for logs.
    pub shown: String,
}

impl TokenKey {
    /// Finds the key and logs in. Errors name the module, token and label,
    /// never the PIN.
    pub fn open(settings: &Pkcs11Settings, spec: &TokenKeySpec) -> Result<Self, String> {
        let module = settings.module.as_deref().ok_or(
            "a pkcs11: signing key needs RUSTICDOC_PKCS11_MODULE, the path of the token's PKCS#11 module",
        )?;
        let mut pin = match &settings.pin_file {
            Some(path) => std::fs::read_to_string(path)
                .map(|p| p.trim_end_matches(['\r', '\n']).to_string())
                .map_err(|e| {
                    format!("cannot read the PKCS#11 PIN file {}: {e}", path.display())
                })?,
            None => settings.pin.clone().ok_or(
                "a pkcs11: signing key needs a PIN: set RUSTICDOC_PKCS11_PIN_FILE (or RUSTICDOC_PKCS11_PIN)",
            )?,
        };
        let context = context(module)?;
        let token_label = spec.token.as_ref().or(settings.token.as_ref());
        let slot_id = spec.slot.or(settings.slot);
        let slots = context
            .get_slots_with_token()
            .map_err(|e| format!("cannot list the PKCS#11 slots: {e}"))?;
        let mut candidates: Vec<(Slot, String)> = Vec::new();
        for slot in slots {
            let label = context
                .get_token_info(slot)
                .map(|t| t.label().trim().to_string())
                .unwrap_or_default();
            if slot_id.is_some_and(|id| slot.id() != id) {
                continue;
            }
            if token_label.is_some_and(|l| *l != label) {
                continue;
            }
            candidates.push((slot, label));
        }
        if candidates.is_empty() {
            wipe(&mut pin);
            return Err(match (token_label, slot_id) {
                (Some(l), _) => format!("no PKCS#11 token is labelled '{l}'"),
                (None, Some(id)) => format!("PKCS#11 slot {id} has no token"),
                (None, None) => "the PKCS#11 module has no token".into(),
            });
        }
        let auth = AuthPin::new(pin.clone().into());
        wipe(&mut pin);
        let mut problems = Vec::new();
        for (slot, label) in candidates {
            let session = match context.open_ro_session(slot) {
                Ok(s) => s,
                Err(e) => {
                    problems.push(format!("token '{label}': cannot open a session: {e}"));
                    continue;
                }
            };
            match session.login(UserType::User, Some(&auth)) {
                Ok(()) | Err(CkError::Pkcs11(RvError::UserAlreadyLoggedIn, _)) => {}
                Err(e) => {
                    problems.push(format!("token '{label}': cannot log in: {e}"));
                    continue;
                }
            }
            let found = session
                .find_objects(&[
                    Attribute::Class(ObjectClass::PRIVATE_KEY),
                    Attribute::Label(spec.label.as_bytes().to_vec()),
                ])
                .map_err(|e| format!("token '{label}': cannot search for keys: {e}"))?;
            match found.as_slice() {
                [key] => {
                    return Ok(Self {
                        key: *key,
                        session: Mutex::new(session),
                        shown: format!("'{}' on token '{label}'", spec.label),
                    });
                }
                [] => problems.push(format!(
                    "token '{label}' has no private key labelled '{}'",
                    spec.label
                )),
                more => problems.push(format!(
                    "token '{label}' has {} private keys labelled '{}'; labels must be unique",
                    more.len(),
                    spec.label
                )),
            }
        }
        Err(problems.join("; "))
    }

    /// Signs `data` with `mechanism`.
    pub fn sign(&self, mechanism: &Mechanism, data: &[u8]) -> Result<Vec<u8>, String> {
        let session = self.session.lock().unwrap_or_else(|e| e.into_inner());
        session
            .sign(mechanism, self.key, data)
            .map_err(|e| format!("the PKCS#11 token refused to sign with {}: {e}", self.shown))
    }
}

fn wipe(s: &mut str) {
    // SAFETY: zeroes are valid UTF-8, so the string stays valid.
    for b in unsafe { s.as_bytes_mut() } {
        // SAFETY: `b` is a valid, aligned, exclusive reference to one byte.
        unsafe { std::ptr::write_volatile(b, 0) };
    }
}

#[cfg(test)]
mod tests {
    use super::TokenKeySpec;

    #[test]
    fn key_specs_name_a_label_and_optionally_a_token() {
        assert_eq!(
            TokenKeySpec::parse("signer").unwrap(),
            TokenKeySpec {
                label: "signer".into(),
                token: None,
                slot: None
            }
        );
        assert_eq!(
            TokenKeySpec::parse("object=my%20key;token=prod;slot-id=3;type=private").unwrap(),
            TokenKeySpec {
                label: "my key".into(),
                token: Some("prod".into()),
                slot: Some(3)
            }
        );
        assert!(TokenKeySpec::parse("").is_err());
        assert!(TokenKeySpec::parse("token=prod").is_err());
        assert!(TokenKeySpec::parse("object=x;colour=blue").is_err());
    }
}
