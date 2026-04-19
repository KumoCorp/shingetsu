use std::fmt;

pub struct Plural<'a> {
    count: usize,
    word: &'a str,
}

impl fmt::Display for Plural<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.count == 1 {
            write!(f, "{}", self.word)
        } else {
            write!(f, "{}s", self.word)
        }
    }
}

pub fn plural(count: usize, word: &str) -> Plural<'_> {
    Plural { count, word }
}
