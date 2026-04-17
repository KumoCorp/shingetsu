use shingetsu::FromLua;

#[derive(FromLua)]
enum Bad {
    Nothing,
    Str(bytes::Bytes),
}

fn main() {}
