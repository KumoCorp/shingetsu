use shingetsu::FromLua;

#[derive(FromLua)]
enum Bad {
    Multi(i64, String),
    Str(shingetsu::Bytes),
}

fn main() {}
