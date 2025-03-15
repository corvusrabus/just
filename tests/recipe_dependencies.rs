use super::*;

#[test]
fn recipe_dependency_nested_module() {
  Test::new()
    .write("foo.just", "mod bar\nbaz: \n @echo FOO")
    .write("bar.just", "baz:\n @echo BAZ")
    .justfile(
      "
      mod foo

      baz: foo::bar::baz
      ",
    )
    .arg("baz")
    .stdout("BAZ\n")
    .run();
}
