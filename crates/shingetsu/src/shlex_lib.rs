//! POSIX-shell word splitting and quoting for the standard library.
//!
//! These build and take apart shell command lines safely -- quoting
//! untrusted data so a shell reads it back as literal words instead
//! of interpreting metacharacters.  They perform no I/O, so they are
//! available even in sandboxed environments: composing a command line
//! is useful whether or not the caller is allowed to run it.  Every
//! function operates on raw bytes, so non-UTF-8 strings are handled
//! without loss.

use crate::value::Value;
use crate::VmError;
use shingetsu::Bytes;

/// Build the `shlex` library table and register it as the `shlex`
/// global.
pub fn register(env: &crate::GlobalEnv) -> Result<(), VmError> {
    let table = shlex_mod::build_module_table(env)?;
    env.set_global("shlex", Value::Table(table));
    env.register_module_type("shlex", shlex_mod::module_type());
    Ok(())
}

/// Build a `BadArgument` for a value that `shlex.quote` or
/// `shlex.join` refuses to quote.  `got` describes the offending
/// input; the whole value is call argument #1.
fn quote_reject(function: &str, got: String) -> VmError {
    VmError::BadArgument {
        position: 1,
        function: function.to_owned(),
        expected: "a string without a nul byte".to_owned(),
        got,
    }
}

/// Safe POSIX-shell word splitting and quoting for building command
/// lines from untrusted input.
///
/// `shlex.split` parses a command line into an argument list;
/// `shlex.quote` and `shlex.join` escape strings so a shell reads
/// them back as single, literal words.  All three operate on raw
/// bytes and are available in sandboxed environments.
#[crate::module(name = "shlex")]
pub mod shlex_mod {
    use super::*;

    /// Split a command line into a list of arguments using POSIX
    /// shell word-splitting rules (whitespace separates words; single
    /// and double quotes and backslashes group and escape).
    ///
    /// Returns a sequence of the parsed words on success.  When the
    /// input is malformed -- it ends inside an unbalanced quote or
    /// with a trailing unescaped backslash -- returns `nil` plus an
    /// error message rather than raising, since splitting untrusted
    /// input is an expected, recoverable operation.
    ///
    /// # Parameters
    ///
    /// - `s` -- the command line to split
    ///
    /// # Returns
    ///
    /// - a sequence of argument strings on success
    /// - `nil` plus an error message when the input is malformed
    ///
    /// An empty or whitespace-only input splits to an empty list, not
    /// an error.
    ///
    /// # Examples
    ///
    /// ```lua
    /// local args = shlex.split('foo "bar baz" qux')
    /// assert(#args == 3)
    /// assert(args[2] == "bar baz")
    /// ```
    ///
    /// ```lua
    /// -- A malformed command line is reported, not raised.
    /// local args, err = shlex.split('unterminated "quote')
    /// assert(args == nil)
    /// assert(err == "input ends in an unbalanced quote or trailing backslash")
    /// ```
    #[function]
    fn split(s: Bytes) -> Result<crate::convert::StdlibResult<Vec<Bytes>>, VmError> {
        match shlex::bytes::split(&s) {
            Some(words) => Ok(crate::convert::StdlibResult::Ok(
                words.into_iter().map(Bytes::from).collect(),
            )),
            None => Ok(crate::convert::StdlibResult::Err(
                "input ends in an unbalanced quote or trailing backslash".to_owned(),
            )),
        }
    }

    /// Quote a single string so that a POSIX shell reads it back as
    /// one literal word.
    ///
    /// A string that needs no quoting is returned unchanged; the
    /// empty string becomes `''`.  Raises if `s` contains a nul byte,
    /// which can never appear in a shell argument.
    ///
    /// # Parameters
    ///
    /// - `s` -- the string to quote
    ///
    /// # Returns
    ///
    /// - the shell-quoted string
    ///
    /// # Examples
    ///
    /// ```lua
    /// assert(shlex.quote("a b") == "'a b'")
    /// assert(shlex.quote("") == "''")
    /// assert(shlex.quote("plain") == "plain")
    /// ```
    #[function]
    fn quote(s: Bytes) -> Result<Bytes, VmError> {
        match shlex::bytes::try_quote(&s) {
            Ok(quoted) => Ok(Bytes::from(quoted.into_owned())),
            Err(shlex::QuoteError::Nul) => {
                Err(quote_reject("shlex.quote", "a nul byte".to_owned()))
            }
            // `QuoteError` is non-exhaustive; report any future
            // rejection by its own description rather than assuming a
            // nul byte.
            Err(other) => Err(quote_reject("shlex.quote", other.to_string())),
        }
    }

    /// Quote and space-join a list of arguments into a single command
    /// line that a POSIX shell splits back into the original words.
    ///
    /// The inverse of `shlex.split` for well-formed argument lists.
    /// Raises if any argument contains a nul byte.
    ///
    /// # Parameters
    ///
    /// - `argv` -- a sequence of argument strings
    ///
    /// # Returns
    ///
    /// - the joined, shell-quoted command line
    ///
    /// # Examples
    ///
    /// ```lua
    /// assert(shlex.join({"echo", "a b", "c"}) == "echo 'a b' c")
    /// ```
    #[function]
    fn join(argv: Vec<Bytes>) -> Result<Bytes, VmError> {
        match shlex::bytes::try_join(argv.iter().map(|a| a.as_ref())) {
            Ok(joined) => Ok(Bytes::from(joined)),
            Err(shlex::QuoteError::Nul) => {
                // `try_join` does not report which element held the
                // nul, so locate it here and name it in the message.
                let got = match argv.iter().position(|a| a.as_ref().contains(&0)) {
                    Some(i) => format!("a nul byte in element {}", i + 1),
                    None => "a nul byte".to_owned(),
                };
                Err(quote_reject("shlex.join", got))
            }
            // `QuoteError` is non-exhaustive; report any future
            // rejection by its own description rather than assuming a
            // nul byte.
            Err(other) => Err(quote_reject("shlex.join", other.to_string())),
        }
    }
}
