use shingetsu::FromLua;

#[derive(FromLua)]
struct Foo {
    x: i64,
}

#[derive(FromLua)]
struct Bar {
    y: i64,
}

#[derive(FromLua)]
enum Bad {
    A(Foo),
    B(Bar),
}

fn main() {}
