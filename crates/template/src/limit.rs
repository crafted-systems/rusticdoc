//! Rendering with a ceiling on the bytes written.
//!
//! Fuel counts instructions, and printing a long value is one instruction, so
//! fuel alone lets a short loop ask for terabytes. Every render goes through
//! [`render`], which stops at the first write past the limit.

use std::io;

use minijinja::value::Value;
use minijinja::{Error, Template};

/// Why a limited render stopped.
#[derive(Debug)]
pub enum RenderFailure {
    /// The output would have passed the limit, in bytes.
    TooLarge(usize),
    /// The template itself failed.
    Template(Error),
}

struct Limited<'a> {
    out: &'a mut Vec<u8>,
    limit: usize,
    exceeded: &'a mut bool,
}

impl io::Write for Limited<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.out.len().saturating_add(buf.len()) > self.limit {
            *self.exceeded = true;
            return Err(io::Error::other("the render limit was passed"));
        }
        self.out.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Renders into a buffer of at most `limit` bytes, and hands back the output
/// with the render's state.
pub fn render<'env, 'source>(
    template: &Template<'env, 'source>,
    ctx: Value,
    limit: usize,
) -> Result<(String, minijinja::Captured<'source>), RenderFailure> {
    let mut out = Vec::new();
    let mut exceeded = false;
    let writer = Limited {
        out: &mut out,
        limit,
        exceeded: &mut exceeded,
    };
    match template.render_captured_to(ctx, writer) {
        Ok(captured) => {
            let text = String::from_utf8(out)
                .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned());
            Ok((text, captured))
        }
        Err(_) if exceeded => Err(RenderFailure::TooLarge(limit)),
        Err(e) => Err(RenderFailure::Template(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use minijinja::Environment;

    #[test]
    fn a_short_loop_cannot_write_past_the_limit() {
        let mut env = Environment::new();
        env.set_fuel(Some(5_000_000));
        let template = env
            .template_from_str(
                r#"{% set x = "a" * 1000 %}{% for i in range(100000) %}{{ x }}{% endfor %}"#,
            )
            .unwrap();
        let result = render(&template, Value::UNDEFINED, 1 << 20);
        assert!(matches!(result, Err(RenderFailure::TooLarge(_))));
    }

    #[test]
    fn output_within_the_limit_comes_back_whole_with_its_state() {
        let env = Environment::new();
        let template = env.template_from_str("{% set y = 2 %}héllo").unwrap();
        let (text, captured) = render(&template, Value::UNDEFINED, 6).unwrap();
        assert_eq!(text, "héllo");
        assert_eq!(captured.state().lookup("y"), Some(Value::from(2)));
    }
}
