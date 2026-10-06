# fairway-template

The crate turns template text and a set of values into a finished string.
Data is passed as a regular struct or map implementing `Serialize`.

`Template::compile` prepares a template, and `Template::render` executes
it with the supplied values. A prepared template can be reused many times.
Both operations are synchronous.

Plugin dependencies:

```toml
[dependencies]
fairway-codec.workspace = true
fairway-template.workspace = true
```

## Quick start

```rust
use fairway_codec::serde::Serialize;
use fairway_template::{Error, Template};

#[derive(Serialize)]
#[serde(crate = "fairway_codec::serde")]
struct Report<'a> {
    branch: &'a str,
    files: &'a [&'a str],
}

fn report() -> Result<String, Error> {
    let template = Template::compile(
        "report",
        "Branch: {{ branch }}\nFiles: {{ files | join(', ') }}\n",
    )?;

    template.render(&Report {
        branch: "main",
        files: &["one.rs", "two.rs"],
    })
}
```

Result:

```text
Branch: main
Files: one.rs, two.rs
```

`compile` parses the source text, checks its syntax, and prepares instructions
for execution. Calls to `render` do not repeat this work.

## Context

A context is a set of named values available to the template. The `branch`
field is available as `{{ branch }}`, and a nested field as `{{ project.name }}`.
`render` accepts either a value or a reference to it. Contexts from previous
calls are not retained.

Context names can shadow MiniJinja's built-in functions.
For example, a field named `range` hides the function of the same name.

## Optional data

The fields in a context can be determined at runtime. In this example,
the title is added only when it is supplied:

```rust
use std::collections::BTreeMap;

use fairway_template::{Error, Template};

fn heading(title: Option<&str>) -> Result<String, Error> {
    let template = Template::compile(
        "heading",
        "{{ title | default('Untitled') }}",
    )?;

    let mut context = BTreeMap::new();
    if let Some(title) = title {
        context.insert("title", title);
    }

    template.render(&context)
}
```

`heading(Some("Report"))` returns `"Report"`, and `heading(None)` returns
`"Untitled"`. Both cases use the same `render` method.

If there are no input variables, pass an empty map or call `render(())`.

Templates follow MiniJinja's rules in `Strict` mode. If a template outputs
`{{ title }}` but the context has no `title` field, `render` returns an error
of kind `ErrorKind::Undefined`.

The `default` filter with one argument supplies fallback text only
for a missing field:

| Supplied fields | Result of `{{ title \| default('Untitled') }}` |
|---|---|
| No `title` field | `Untitled` |
| `title` is `"Report"` | `Report` |
| `title` is `""` | Empty string |
| `title` is `null` | `None` |

To use the fallback text for an empty string or `null` as well, use
`{{ title | default('Untitled', true) }}`. This also replaces
`false`, `0`, and empty collections.

## Where the data comes from

The caller prepares the data. The API does not allow registering caller-defined
functions or data providers. The language's built-in functions and filters
are available.

Before the template executes, the entire context is serialized according to
Serde's JSON rules. Serialization failures return `ErrorKind::Data`,
even if the template does not use the problematic field.

## Template language

Template syntax is described in the [MiniJinja guide](https://docs.rs/minijinja/latest/minijinja/syntax/index.html).

The API does not support loading other templates. Independently created
`Template` objects cannot see each other. The `include`, `import`, and `extends`
syntax is accepted as part of MiniJinja, but attempting to retrieve a missing
template during rendering returns `ErrorKind::Render`. An explicit
`include ... ignore missing` skips a missing template. None of these constructs
reads files.

## How text is handled

Automatic escaping is disabled by default, regardless of the template name.

Whitespace and line endings are preserved, including the final newline
and CRLF, unless the template author explicitly specifies their removal.

## Errors

`compile` and `render` return `fairway_template::Error`.
The message includes the template name and, when the error's location is
known, the line number:

```text
report:3: cannot render template: undefined value
```

For handling errors in code, use `kind()`, `template_name()`, and `line()`.
Line numbers start at one; context errors have no line number.
If there is an underlying error, it is available through
`std::error::Error::source()`.

| `ErrorKind` | What happened |
|---|---|
| `InvalidName` | The name is empty or contains a control character |
| `Syntax` | A template syntax error |
| `Data` | A serialization failure or an invalid context type |
| `Undefined` | MiniJinja classified the error as the use of an undefined value |
| `Render` | Another evaluation error, such as invalid operands or an unknown filter |

`Error` implements `std::error::Error + Send + Sync` and owns its diagnostic
information. `ErrorKind` is declared as `#[non_exhaustive]`, so a match
covering all categories needs a final `_` arm.

No partial string is returned on error. The template remains usable
for subsequent calls with different data.
