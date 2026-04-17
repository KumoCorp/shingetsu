use shingetsu::FromLua;

#[derive(FromLua)]
enum Bad {
    A(i64),
    B(i32),
}

fn main() {}
