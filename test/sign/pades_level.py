"""Prints the PAdES baseline level pyHanko confirms for each file's first
signature, with nothing fetched from the network:

  B-B    the signature is intact and its certificate chains to the root
  B-T    and it carries a signature timestamp pyHanko validates
  B-LT   and it validates with revocation checking on (hard-fail), from the
         DSS alone: the certificates, OCSP responses and CRLs in the file
  B-LTA  and a document timestamp over a revision with that DSS validates the
         same way

pyHanko 0.38 has no single "AdES level" verdict, so this builds one from its
validators. Run it with the timestamp authority and revocation servers
stopped, so nothing can come from them.

Usage: python -I test/sign/pades_level.py ROOT.pem FILE.pdf...
"""

import sys

from pyhanko.keys import load_cert_from_pemder
from pyhanko.pdf_utils.reader import PdfFileReader
from pyhanko.sign.validation import validate_pdf_signature, validate_pdf_timestamp
from pyhanko.sign.validation.dss import DocumentSecurityStore
from pyhanko_certvalidator import ValidationContext


def ok(status):
    return status.intact and status.valid and status.trusted


def level(path, root):
    with open(path, "rb") as f:
        r = PdfFileReader(f)
        sig = r.embedded_regular_signatures[0]
        soft = ValidationContext(
            trust_roots=[root], allow_fetching=False, revocation_mode="soft-fail"
        )
        status = validate_pdf_signature(sig, soft, soft)
        if not ok(status):
            return "none", status.summary()
        ts = status.timestamp_validity
        if ts is None or not ok(ts):
            return "B-B", status.summary()
        if "/DSS" not in r.root:
            return "B-T", status.summary()

        def offline():
            return DocumentSecurityStore.read_dss(r).as_validation_context(
                {
                    "trust_roots": [root],
                    "allow_fetching": False,
                    "revocation_mode": "hard-fail",
                }
            )

        hard = offline()
        status = validate_pdf_signature(sig, hard, hard)
        if not status.bottom_line:
            return "B-T", "the DSS does not suffice: " + status.summary()
        result = ("B-LT", status.summary())
        for doc_ts in r.embedded_timestamp_signatures:
            covered_root = r.get_historical_root(doc_ts.signed_revision)
            if "/DSS" not in covered_root:
                continue
            ts_status = validate_pdf_timestamp(doc_ts, offline())
            if ok(ts_status):
                result = ("B-LTA", status.summary() + "; document timestamp " + ts_status.summary())
            else:
                return "B-LT", "document timestamp: " + ts_status.summary()
        return result


def main():
    root = load_cert_from_pemder(sys.argv[1])
    for path in sys.argv[2:]:
        found, detail = level(path, root)
        print(f"{path}: {found} ({detail})")


if __name__ == "__main__":
    main()
