//! Reusable text templates with explicit rendering contexts.
//!
//! Compile a MiniJinja template once, then render it with a serializable struct
//! or map. Its fields are available directly as template variables.
//!
//! ```
//! use fairway_codec::serde::Serialize;
//! use fairway_template::Template;
//!
//! #[derive(Serialize)]
//! #[serde(crate = "fairway_codec::serde")]
//! struct Report<'a> {
//!     branch: &'a str,
//!     files: &'a [&'a str],
//! }
//!
//! let template = Template::compile(
//!     "report",
//!     "Ветка: {{ branch }}\nФайлы: {{ files | join(', ') }}\n",
//! )?;
//! let report = Report { branch: "main", files: &["one.rs", "two.rs"] };
//! assert_eq!(
//!     template.render(&report)?,
//!     "Ветка: main\nФайлы: one.rs, two.rs\n",
//! );
//! # Ok::<(), fairway_template::Error>(())
//! ```
//!
//! Rendering is synchronous and follows MiniJinja's `Strict` undefined behavior.
//! This mode does not reject every use of an undefined value; operators and
//! filters follow MiniJinja's semantics. Automatic escaping is disabled; literal
//! whitespace and trailing newlines are preserved. Each template is independent
//! and has no loader for other templates or files.

mod error;

use std::sync::Arc;

use fairway_codec::{
    json::{self, Value},
    serde::Serialize,
};
use minijinja::{AutoEscape, Environment, UndefinedBehavior};

pub use error::{Error, ErrorKind};

/// An owned, compiled template that can be rendered repeatedly.
///
/// Clones share the prepared template. Rendering does not retain the context or
/// output, and clones can be used concurrently on different threads.
#[derive(Clone)]
pub struct Template {
    inner: Arc<Inner>,
}

struct Inner {
    name: String,
    environment: Environment<'static>,
}

impl Template {
    /// Compiles a template and retains its source and diagnostic name.
    ///
    /// The name must be nonempty and contain no control characters. It does not
    /// open a file or select an output format. Source strings may be dropped
    /// after this call; parsing and compilation are not repeated by [`render`](Self::render).
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::InvalidName`] for an invalid name or
    /// [`ErrorKind::Syntax`] for invalid template syntax.
    pub fn compile(name: impl Into<String>, source: impl Into<String>) -> Result<Self, Error> {
        let name = name.into();
        if name.is_empty() {
            return Err(Error::invalid_name(name, "name must not be empty"));
        }
        if name.chars().any(char::is_control) {
            return Err(Error::invalid_name(
                name,
                "name must not contain control characters",
            ));
        }

        let mut environment = Environment::new();
        environment.set_undefined_behavior(UndefinedBehavior::Strict);
        environment.set_auto_escape_callback(|_| AutoEscape::None);
        environment.set_keep_trailing_newline(true);
        environment.set_trim_blocks(false);
        environment.set_lstrip_blocks(false);
        environment.set_debug(false);
        environment
            .add_template_owned(name.clone(), source.into())
            .map_err(|source| Error::syntax(&name, source))?;

        Ok(Self {
            inner: Arc::new(Inner { name, environment }),
        })
    }

    /// Executes the template with a serializable context.
    ///
    /// The entire context is serialized through `fairway-codec` according to
    /// Serde's JSON rules before the template executes. Its root must be an
    /// object or `null`. A null root (including `()` and `Option::None`) supplies
    /// no variables but still runs the template. Other root types return
    /// [`ErrorKind::Data`].
    ///
    /// Pass a reference to retain ownership of the context. Borrowed fields and
    /// contexts that are not `Send`, `Sync`, or `Clone` are supported.
    ///
    /// ```
    /// use fairway_template::Template;
    /// use std::collections::BTreeMap;
    ///
    /// let template = Template::compile("heading", "{{ title | default('Без названия') }}")?;
    /// let context = BTreeMap::from([("title", "Отчёт")]);
    /// assert_eq!(template.render(&context)?, "Отчёт");
    /// assert_eq!(template.render(())?, "Без названия");
    /// for context in [Some(&context), None] {
    ///     let rendered = template.render(context)?;
    ///     assert_eq!(rendered, if context.is_some() { "Отчёт" } else { "Без названия" });
    /// }
    /// # Ok::<(), fairway_template::Error>(())
    /// ```
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Data`] for serialization failures or invalid context
    /// roots, [`ErrorKind::Undefined`] when MiniJinja classifies the failure as an
    /// undefined value error, and [`ErrorKind::Render`] for other evaluation
    /// failures. No partial output is returned, and the template can be used
    /// again after an error.
    pub fn render<C: Serialize>(&self, context: C) -> Result<String, Error> {
        let context = json::value::to_value(context)
            .map_err(|source| Error::serialization(self.name(), source))?;
        let context = match context {
            Value::Object(_) => context,
            Value::Null => Value::Object(Default::default()),
            Value::Bool(_) => return Err(Error::context_type(self.name(), "boolean")),
            Value::Number(_) => return Err(Error::context_type(self.name(), "number")),
            Value::String(_) => return Err(Error::context_type(self.name(), "string")),
            Value::Array(_) => return Err(Error::context_type(self.name(), "array")),
        };

        let template = self
            .inner
            .environment
            .get_template(self.name())
            .map_err(|source| Error::render(self.name(), source))?;
        template
            .render(context)
            .map_err(|source| Error::render(self.name(), source))
    }

    /// Returns the diagnostic name supplied to [`compile`](Self::compile).
    pub fn name(&self) -> &str {
        &self.inner.name
    }
}
