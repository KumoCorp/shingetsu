use shingetsu::FromLua;

#[derive(FromLua)]
enum Bad {
    Nothing,
    Str(shingetsu::Bytes),
}

fn main() {}
