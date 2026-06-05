use shingetsu::LuaRepr;

// `try_from` delegates the whole struct to an intermediate type, so the
// intermediate's field names are what reach lua.  A `rename_all` on Self
// would never apply to anything; reject it at compile time rather than
// silently doing nothing.
#[derive(Debug, Clone, PartialEq, LuaRepr)]
struct Wire {
    max_retries: i64,
}

#[derive(Debug, Clone, PartialEq, LuaRepr)]
#[lua(try_from = "Wire", rename_all = "kebab-case")]
struct Config {
    max_retries: i64,
}

impl TryFrom<Wire> for Config {
    type Error = ::std::convert::Infallible;
    fn try_from(w: Wire) -> Result<Self, Self::Error> {
        Ok(Self {
            max_retries: w.max_retries,
        })
    }
}

impl From<Config> for Wire {
    fn from(c: Config) -> Self {
        Self {
            max_retries: c.max_retries,
        }
    }
}

fn main() {}
