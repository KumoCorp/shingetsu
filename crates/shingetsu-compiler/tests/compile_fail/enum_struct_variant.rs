use shingetsu::FromLua;

#[derive(FromLua)]
enum Bad {
    Named { x: i64 },
    Str(shingetsu::Bytes),
}

fn main() {}
