use {super::*, CompileErrorKind::*};

pub struct ResolvedRecipes<'src> {
  name: &'src str,
  here: Table<'src, Rc<Recipe<'src>>>,
  modules: Table<'src, ResolvedRecipes<'src>>,
}

impl<'src> ResolvedRecipes<'src> {
  pub fn values_here(&self) -> impl Iterator<Item = &Rc<Recipe<'src>>> {
    self.here.values()
  }

  pub fn get_here(&self, name: &str) -> Option<&Rc<Recipe<'src>>> {
    self.here.get(name)
  }

  pub fn get_path(&self, path: Namepath) -> Option<&Rc<Recipe<'src>>> {
    let (name, remaining_path) = path.split_first();
    let name = name.lexeme();
    if remaining_path.is_empty() {
      self.get_here(name)
    } else {
      self
        .modules
        .get(name)
        .and_then(|module| module.get_slice_path(remaining_path))
    }
  }

  fn get_slice_path(&self, path: &[Name]) -> Option<&Rc<Recipe<'src>>> {
    let (name, remaining_path) = path
      .split_first()
      .expect("Internal error: method should not be called with an empty path");
    let name = name.lexeme();

    if remaining_path.is_empty() {
      self.get_here(name)
    } else {
      self
        .modules
        .get(name)
        .and_then(|module| module.get_slice_path(remaining_path))
    }
  }
}

impl<'src> Keyed<'src> for ResolvedRecipes<'src> {
  fn key(&self) -> &'src str {
    self.name
  }
}

pub(crate) struct RecipeResolver<'src: 'run, 'run> {
  assignments: &'run Table<'src, Assignment<'src>>,
  resolved_recipes: ResolvedRecipes<'src>,
  unresolved_recipes: Table<'src, UnresolvedRecipe<'src>>,
}

impl<'src: 'run, 'run> RecipeResolver<'src, 'run> {
  pub(crate) fn resolve_recipes(
    assignments: &'run Table<'src, Assignment<'src>>,
    settings: &Settings,
    unresolved_recipes: Table<'src, UnresolvedRecipe<'src>>,
    resolved_recipes: ResolvedRecipes<'src>,
  ) -> CompileResult<'src, ResolvedRecipes<'src>> {
    let mut resolver = Self {
      resolved_recipes,
      unresolved_recipes,
      assignments,
    };

    while let Some(unresolved) = resolver.unresolved_recipes.pop() {
      resolver.resolve_recipe(&mut Vec::new(), unresolved)?;
    }

    for recipe in resolver.resolved_recipes.values_here() {
      for (i, parameter) in recipe.parameters.iter().enumerate() {
        if let Some(expression) = &parameter.default {
          for variable in expression.variables() {
            resolver.resolve_variable(&variable, &recipe.parameters[..i])?;
          }
        }
      }

      for dependency in &recipe.dependencies {
        for argument in &dependency.arguments {
          for variable in argument.variables() {
            resolver.resolve_variable(&variable, &recipe.parameters)?;
          }
        }
      }

      for line in &recipe.body {
        if line.is_comment() && settings.ignore_comments {
          continue;
        }

        for fragment in &line.fragments {
          if let Fragment::Interpolation { expression, .. } = fragment {
            for variable in expression.variables() {
              resolver.resolve_variable(&variable, &recipe.parameters)?;
            }
          }
        }
      }
    }

    Ok(resolver.resolved_recipes)
  }

  fn resolve_variable(
    &self,
    variable: &Token<'src>,
    parameters: &[Parameter],
  ) -> CompileResult<'src> {
    let name = variable.lexeme();

    let defined = self.assignments.contains_key(name)
      || parameters.iter().any(|p| p.name.lexeme() == name)
      || constants().contains_key(name);

    if !defined {
      return Err(variable.error(UndefinedVariable { variable: name }));
    }

    Ok(())
  }

  fn resolve_recipe(
    &mut self,
    stack: &mut Vec<&'src str>,
    recipe: UnresolvedRecipe<'src>,
  ) -> CompileResult<'src, Rc<Recipe<'src>>> {
    if let Some(resolved) = self.resolved_recipes.get_here(recipe.name()) {
      return Ok(Rc::clone(resolved));
    }

    stack.push(recipe.name());

    let mut dependencies: Vec<Rc<Recipe>> = Vec::new();
    for dependency in &recipe.dependencies {
      let path = dependency.recipe;

      if let Some(resolved) = self.resolved_recipes.get_path(path) {
        // dependency already resolved
        dependencies.push(Rc::clone(resolved));
      } else if stack.contains(&path) {
        let first = stack[0];
        stack.push(first);
        return Err(
          dependency.recipe.last().error(CircularRecipeDependency {
            recipe: recipe.name(),
            circle: stack
              .iter()
              .skip_while(|name| **name != dependency.recipe.last().lexeme())
              .copied()
              .collect(),
          }),
        );
      } else if let Some(unresolved) = self.unresolved_recipes.remove(path) {
        // resolve unresolved dependency
        dependencies.push(self.resolve_recipe(stack, unresolved)?);
      } else {
        // dependency is unknown
        return Err(dependency.recipe.last().error(UnknownDependency {
          recipe: dependency.recipe.clone(),
          unknown: path,
        }));
      }
    }

    stack.pop();

    let resolved = Rc::new(recipe.resolve(dependencies)?);
    self.resolved_recipes.insert(Rc::clone(&resolved));
    Ok(resolved)
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  analysis_error! {
    name:   circular_recipe_dependency,
    input:  "a: b\nb: a",
    offset: 8,
    line:   1,
    column: 3,
    width:  1,
    kind:   CircularRecipeDependency{recipe: "b", circle: vec!["a", "b", "a"]},
  }

  analysis_error! {
    name:   self_recipe_dependency,
    input:  "a: a",
    offset: 3,
    line:   0,
    column: 3,
    width:  1,
    kind:   CircularRecipeDependency{recipe: "a", circle: vec!["a", "a"]},
  }

  analysis_error! {
    name:   unknown_dependency,
    input:  "a: b",
    offset: 3,
    line:   0,
    column: 3,
    width:  1,
    kind:   UnknownDependency{
      recipe: Namepath::from(Name::from_identifier(
        Token{
          column: 3,
          kind: TokenKind::Identifier,
          length: 1,
          line: 0,
          offset: 3,
          path: &Path::new("justfile"),
          src: "a: b" })),
          unknown: "b"
    },
  }

  analysis_error! {
    name:   unknown_interpolation_variable,
    input:  "x:\n {{   hello}}",
    offset: 9,
    line:   1,
    column: 6,
    width:  5,
    kind:   UndefinedVariable{variable: "hello"},
  }

  analysis_error! {
    name:   unknown_second_interpolation_variable,
    input:  "wtf:=\"x\"\nx:\n echo\n foo {{wtf}} {{ lol }}",
    offset: 34,
    line:   3,
    column: 16,
    width:  3,
    kind:   UndefinedVariable{variable: "lol"},
  }

  analysis_error! {
    name:   unknown_variable_in_default,
    input:  "a f=foo:",
    offset: 4,
    line:   0,
    column: 4,
    width:  3,
    kind:   UndefinedVariable{variable: "foo"},
  }

  analysis_error! {
    name:   unknown_variable_in_dependency_argument,
    input:  "bar x:\nfoo: (bar baz)",
    offset: 17,
    line:   1,
    column: 10,
    width:  3,
    kind:   UndefinedVariable{variable: "baz"},
  }
}
