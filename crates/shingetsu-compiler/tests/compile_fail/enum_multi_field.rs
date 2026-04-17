use shingetsu::FromLua;

#[derive(FromLua)]
enum Bad {
    Multi(i64, String),
    Str(bytes::Bytes),
}

fn main() {}
