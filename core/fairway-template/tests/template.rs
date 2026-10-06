//! Behavioral checks for the public template API.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::error::Error as _;
use std::rc::Rc;

use fairway_codec::{
    json,
    serde::{self, Serialize},
};
use fairway_template::{Error, ErrorKind, Template};

#[test]
fn renders_borrowed_structs_and_reuses_the_context() {
    #[derive(Serialize)]
    #[serde(crate = "fairway_codec::serde")]
    struct Report<'a> {
        branch: &'a str,
        files: &'a [&'a str],
    }

    let branch = String::from("main");
    let report = Report {
        branch: &branch,
        files: &["one.rs", "two.rs"],
    };
    let template = Template::compile(
        "report",
        "Ветка: {{ branch }}\nФайлы: {{ files | join(', ') }}\n",
    )
    .unwrap();
    let expected = "Ветка: main\nФайлы: one.rs, two.rs\n";
    assert_eq!(template.render(&report).unwrap(), expected);
    assert_eq!(template.render(&report).unwrap(), expected);
    assert_eq!(template.name(), "report");
}

#[test]
fn renders_each_context_independently() {
    #[derive(Serialize)]
    #[serde(crate = "fairway_codec::serde")]
    struct Greeting<'a> {
        name: &'a str,
    }

    let template = Template::compile("greeting", "Привет, {{ name }}!").unwrap();
    let greetings: Result<Vec<_>, _> = ["Анна", "Борис"]
        .into_iter()
        .map(|name| template.render(Greeting { name }))
        .collect();
    assert_eq!(greetings.unwrap(), ["Привет, Анна!", "Привет, Борис!"]);
    assert_eq!(
        template.render(()).unwrap_err().kind(),
        ErrorKind::Undefined
    );
}

#[test]
fn empty_and_optional_contexts_still_execute_the_template() {
    let template = Template::compile(
        "heading",
        "{# hidden comment #}{{ title | default('Без названия') }}:{{ 1 + 2 }}",
    )
    .unwrap();
    let empty = "Без названия:3";
    assert_eq!(template.render(()).unwrap(), empty);
    assert_eq!(template.render(json!(null)).unwrap(), empty);
    assert_eq!(
        template.render(BTreeMap::<String, String>::new()).unwrap(),
        empty
    );
    assert_eq!(
        template.render(None::<BTreeMap<String, String>>).unwrap(),
        empty
    );

    let context = BTreeMap::from([("title", "Отчёт")]);
    for context in [Some(&context), None] {
        let expected = if context.is_some() {
            "Отчёт:3"
        } else {
            empty
        };
        assert_eq!(template.render(context).unwrap(), expected);
    }
}

#[test]
fn context_fields_can_be_added_and_removed_at_runtime() {
    let template = Template::compile("heading", "{{ title | default('Без названия') }}").unwrap();
    let mut context = BTreeMap::new();
    assert_eq!(template.render(&context).unwrap(), "Без названия");
    context.insert("title", "Отчёт");
    assert_eq!(template.render(&context).unwrap(), "Отчёт");
    context.remove("title");
    assert_eq!(template.render(&context).unwrap(), "Без названия");
}

#[test]
fn exposes_named_and_nested_values_without_a_prefix() {
    let template = Template::compile(
        "context",
        "{{ project.name }}:{{ files | join(', ') }}:{{ count }}:{{ active }}:{{ data }}",
    )
    .unwrap();
    assert_eq!(
        template
            .render(json!({
                "project": { "name": "fairway" },
                "files": ["one.rs", "two.rs"],
                "count": 2,
                "active": true,
                "data": "ordinary field",
            }))
            .unwrap(),
        "fairway:one.rs, two.rs:2:True:ordinary field",
    );
}

#[test]
fn rejects_roots_without_named_fields() {
    let template = Template::compile("context", "constant text").unwrap();
    for context in [json!("text"), json!(12), json!(true), json!([])] {
        let error = template.render(context).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Data);
        assert_eq!(error.template_name(), "context");
        assert_eq!(error.line(), None);
        assert!(error.source().is_none());
        assert!(error.to_string().contains("object or null"));
    }
    assert_eq!(
        template.render(Some("text")).unwrap_err().kind(),
        ErrorKind::Data
    );
}

#[test]
fn distinguishes_missing_null_and_empty_fields() {
    let template = Template::compile("default", "{{ title | default('Без названия') }}").unwrap();
    for (context, expected) in [
        (json!({}), "Без названия"),
        (json!({ "title": "Отчёт" }), "Отчёт"),
        (json!({ "title": "" }), ""),
        (json!({ "title": null }), "None"),
        (json!({ "title": false }), "False"),
        (json!({ "title": 0 }), "0"),
    ] {
        assert_eq!(template.render(context).unwrap(), expected);
    }

    #[derive(Serialize)]
    #[serde(crate = "fairway_codec::serde")]
    struct Heading<'a> {
        title: Option<&'a str>,
    }
    assert_eq!(template.render(Heading { title: None }).unwrap(), "None");

    let template =
        Template::compile("default", "{{ title | default('Без названия', true) }}").unwrap();
    for title in [
        json!(null),
        json!(""),
        json!(false),
        json!(0),
        json!([]),
        json!({}),
    ] {
        assert_eq!(
            template.render(json!({ "title": title })).unwrap(),
            "Без названия"
        );
    }
}

#[test]
fn missing_values_fail_only_when_the_expression_is_executed() {
    for source in [
        "{{ missing }}",
        "{% if missing %}yes{% endif %}",
        "{% for item in missing %}{{ item }}{% endfor %}",
        "{{ project.name }}",
    ] {
        let template = Template::compile("strict", source).unwrap();
        assert_eq!(
            template.render(()).unwrap_err().kind(),
            ErrorKind::Undefined
        );
    }

    for source in [
        "{% if false %}{{ missing }}{% endif %}",
        "{% if files is defined and files %}{{ files | join(', ') }}{% endif %}",
    ] {
        assert_eq!(
            Template::compile("guarded", source)
                .unwrap()
                .render(())
                .unwrap(),
            ""
        );
    }
}

#[test]
fn undefined_arithmetic_is_reported_as_render_error() {
    let template = Template::compile("arithmetic", "{{ missing + 1 }}").unwrap();
    assert_eq!(template.render(()).unwrap_err().kind(), ErrorKind::Render);
}

#[test]
fn join_with_an_undefined_item_follows_minijinja() {
    let template = Template::compile("join", "{{ [missing] | join(', ') }}").unwrap();
    assert_eq!(template.render(()).unwrap(), "");
}

#[test]
fn provides_builtins_and_allows_context_fields_to_shadow_them() {
    let template =
        Template::compile("builtin", "{% for i in range(3) %}{{ i }}{% endfor %}").unwrap();
    assert_eq!(template.render(()).unwrap(), "012");
    assert_eq!(
        template
            .render(json!({ "range": "field" }))
            .unwrap_err()
            .kind(),
        ErrorKind::Render
    );
    let template = Template::compile("field", "{{ range }}").unwrap();
    assert_eq!(
        template.render(json!({ "range": "field" })).unwrap(),
        "field"
    );
}

#[test]
fn preserves_literal_text_and_explicit_whitespace_control() {
    for source in ["", "  \t", "a\nb\n", "a\r\nb\r\n", "a\r\n\r\n", "a\rb\r"] {
        assert_eq!(
            Template::compile("plain", source)
                .unwrap()
                .render(())
                .unwrap(),
            source
        );
    }
    let template = Template::compile("crlf", "  {{ text }}\r\n").unwrap();
    assert_eq!(
        template.render(json!({ "text": "hi" })).unwrap(),
        "  hi\r\n"
    );
    let template = Template::compile("blocks", "one\n{% if true %}\n  two\n{% endif %}\n").unwrap();
    assert_eq!(template.render(()).unwrap(), "one\n\n  two\n\n");
    let template = Template::compile("trim", "a\n {%- if true -%} b {%- endif -%}\n c").unwrap();
    assert_eq!(template.render(()).unwrap(), "abc");
}

#[test]
fn keeps_values_literal_and_only_escapes_explicitly() {
    for name in ["text", "report.html", "report.xml"] {
        let template = Template::compile(name, "{{ value }}").unwrap();
        assert_eq!(
            template.render(json!({ "value": "<type>&value" })).unwrap(),
            "<type>&value"
        );
        assert_eq!(
            template
                .render(json!({ "value": "{{ unknown }}" }))
                .unwrap(),
            "{{ unknown }}"
        );
    }
    let template = Template::compile("escaped", "{{ value | escape }}").unwrap();
    assert_eq!(
        template.render(json!({ "value": "<&" })).unwrap(),
        "&lt;&amp;"
    );
    let template = Template::compile("raw", "{% raw %}Пример: {{ name }}{% endraw %}").unwrap();
    assert_eq!(template.render(()).unwrap(), "Пример: {{ name }}");
}

struct Counted<'a> {
    text: &'a str,
    count: Rc<Cell<usize>>,
}

impl Serialize for Counted<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.count.set(self.count.get() + 1);
        serializer.serialize_str(self.text)
    }
}

#[test]
fn snapshots_borrowed_non_send_context_once_per_render() {
    #[derive(Serialize)]
    #[serde(crate = "fairway_codec::serde")]
    struct Context<'a> {
        value: Counted<'a>,
        unused: Counted<'a>,
    }

    let text = String::from("once");
    let count = Rc::new(Cell::new(0));
    let context = Context {
        value: Counted {
            text: &text,
            count: Rc::clone(&count),
        },
        unused: Counted {
            text: &text,
            count: Rc::clone(&count),
        },
    };
    let template = Template::compile("snapshot", "{{ value }} {{ value }}").unwrap();
    assert_eq!(template.render(&context).unwrap(), "once once");
    assert_eq!(count.get(), 2);
    assert_eq!(template.render(&context).unwrap(), "once once");
    assert_eq!(count.get(), 4);
}

struct Failing;

impl Serialize for Failing {
    fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
        Err(serde::ser::Error::custom("cannot serialize this field"))
    }
}

#[test]
fn reports_unused_serialization_failures_before_evaluating_the_template() {
    #[derive(Serialize)]
    #[serde(crate = "fairway_codec::serde")]
    struct Context {
        unused: Failing,
    }

    let template = Template::compile("serialization", "{{ missing }}").unwrap();
    let error = template.render(Context { unused: Failing }).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Data);
    assert_eq!(error.template_name(), "serialization");
    assert_eq!(error.line(), None);
    assert_eq!(
        error.source().unwrap().to_string(),
        "cannot serialize this field"
    );
    assert!(error.to_string().contains("cannot serialize this field"));

    let context = BTreeMap::from([((1, 2), "tuple key")]);
    assert_eq!(
        template.render(context).unwrap_err().kind(),
        ErrorKind::Data
    );
}

#[test]
fn owns_names_and_sources_and_keeps_concurrent_renders_independent() {
    fn send_sync<T: Send + Sync>() {}
    send_sync::<Template>();
    send_sync::<Error>();

    let template = {
        let name = String::from("owned");
        let source = String::from(
            "{% set local = namespace(n=0) %}{% set local.n = local.n + 1 %}{{ local.n }}:{{ value }}",
        );
        Template::compile(name.as_str(), source.as_str()).unwrap()
    };
    let clone = template.clone();
    let first = std::thread::spawn(move || template.render(json!({ "value": "one" })).unwrap());
    let second = std::thread::spawn(move || {
        let context = json!({ "value": "two" });
        assert_eq!(clone.name(), "owned");
        assert_eq!(clone.render(&context).unwrap(), "1:two");
        clone.render(context).unwrap()
    });
    assert_eq!(first.join().unwrap(), "1:one");
    assert_eq!(second.join().unwrap(), "1:two");
}

#[test]
fn reports_owned_diagnostics_and_recovers_after_render_errors() {
    for name in ["", "bad\nname", "bad\tname", "bad\0name", "bad\u{0085}name"] {
        let error = Template::compile(name, "").err().unwrap();
        assert_eq!(error.kind(), ErrorKind::InvalidName);
        assert_eq!(error.template_name(), name);
        assert_eq!(error.line(), None);
        assert!(!error.to_string().contains('\n'));
    }
    assert_eq!(
        Template::compile(" имя с пробелами ", "").unwrap().name(),
        " имя с пробелами "
    );

    let error = Template::compile("syntax", "first line\n{{").err().unwrap();
    assert_eq!(error.kind(), ErrorKind::Syntax);
    assert_eq!(error.line(), Some(2));
    assert!(error.source().is_some());
    assert!(
        error
            .to_string()
            .starts_with("syntax:2: cannot compile template:")
    );

    let template = Template::compile("report", "one\ntwo\n{{ branch }}").unwrap();
    let error = template.render(()).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Undefined);
    assert_eq!(error.template_name(), "report");
    assert_eq!(error.line(), Some(3));
    assert!(error.source().is_some());
    assert!(
        error
            .to_string()
            .starts_with("report:3: cannot render template:")
    );
    assert_eq!(
        template.render(json!({ "branch": "main" })).unwrap(),
        "one\ntwo\nmain"
    );
    drop(template);
    std::thread::spawn(move || {
        assert_eq!(error.template_name(), "report");
        assert!(error.source().is_some());
    })
    .join()
    .unwrap();

    let template = Template::compile("filter", "{{ title | nonexistent_filter }}").unwrap();
    assert_eq!(
        template.render(json!({ "title": "x" })).unwrap_err().kind(),
        ErrorKind::Render
    );
}

#[test]
fn supports_local_macros_and_independent_templates_with_the_same_name() {
    let template = Template::compile(
        "macro",
        "{% macro line(value) %}[{{ value }}]{% endmacro %}{{ line(name) }}",
    )
    .unwrap();
    assert_eq!(
        template.render(json!({ "name": "value" })).unwrap(),
        "[value]"
    );

    let first = Template::compile("same", "one").unwrap();
    let second = Template::compile("same", "two").unwrap();
    assert_eq!(first.render(()).unwrap(), "one");
    assert_eq!(second.render(()).unwrap(), "two");
}

#[test]
fn accepts_external_template_syntax_but_does_not_load_templates_or_files() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("other.j2");
    std::fs::write(&path, "file contents").unwrap();
    let other = Template::compile("other", "other contents").unwrap();
    assert_eq!(other.render(()).unwrap(), "other contents");

    for source in [
        "{% include target %}",
        "{% import target as macros %}",
        "{% extends target %}",
    ] {
        let template = Template::compile("isolated", source).unwrap();
        for target in ["other", path.to_str().unwrap()] {
            assert_eq!(
                template
                    .render(json!({ "target": target }))
                    .unwrap_err()
                    .kind(),
                ErrorKind::Render,
            );
        }
    }

    let template =
        Template::compile("optional", "before{% include target ignore missing %}after").unwrap();
    assert_eq!(
        template
            .render(json!({ "target": path.to_str().unwrap() }))
            .unwrap(),
        "beforeafter"
    );
    let template = Template::compile(
        "unused",
        "{% if false %}{% include 'missing' %}{% endif %}text",
    )
    .unwrap();
    assert_eq!(template.render(()).unwrap(), "text");
}

#[test]
fn preserves_the_origin_of_an_error_in_an_included_invocation() {
    let template = Template::compile(
        "self",
        "{% if nested | default(false) %}\n{{ missing }}\n{% else %}\n\
         {% with nested = true %}{% include 'self' %}{% endwith %}\n{% endif %}",
    )
    .unwrap();
    let error = template.render(()).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Undefined);
    assert_eq!(error.template_name(), "self");
    assert_eq!(error.line(), Some(2));
    assert!(error.source().unwrap().source().is_some());
}
