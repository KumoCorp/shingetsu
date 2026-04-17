use shingetsu::FromLua;

#[derive(FromLua)]
enum Bad {
    Named { x: i64 },
    Str(bytes::Bytes),
}

fn main() {}
