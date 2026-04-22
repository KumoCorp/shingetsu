use shingetsu::FromLua;

#[derive(FromLua)]
enum Bad {
    MaybeInt(Option<i64>),
    Str(shingetsu::Bytes),
}

fn main() {}
