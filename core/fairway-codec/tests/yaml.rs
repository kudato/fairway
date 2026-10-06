//! YAML values, document boundaries, and diagnostics through the codec API.

use std::{collections::BTreeMap, error::Error as _};

use fairway_codec::{Decode, Encode, Error, Yaml, json, yaml};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
struct Skill {
    name: String,
    description: String,
    metadata: BTreeMap<String, String>,
}

#[test]
fn typed_documents_round_trip_with_owned_and_borrowed_values() {
    let input = "# Instructions\nname: commit\ndescription: |\n  Подготовь коммит.\n  Проверь изменения.\nmetadata:\n  fairway-template: \"false\"\n";
    let mut skill = Yaml::<Skill>::decode(input.as_bytes().to_vec()).unwrap();
    assert_eq!(
        skill.as_ref().description,
        "Подготовь коммит.\nПроверь изменения.\n"
    );
    assert_eq!(skill.0.metadata["fairway-template"], "false");
    skill.0.name = "team-commit".into();
    let bytes = Yaml(&skill.0).encode().unwrap();
    assert_eq!(Yaml::<Skill>::decode(bytes).unwrap(), skill);
    let expected = skill.0.clone();
    let bytes = skill.encode().unwrap();
    assert_eq!(Yaml::<Skill>::decode(bytes).unwrap().into_inner(), expected);
}

#[test]
fn default_values_use_codec_json_types_and_strict_booleans() {
    let mut value: Yaml = Yaml::decode(
        b"name: fairway\nenabled: true\ndisabled: false\nanswer: no\ncount: 2\noptional: null\nitems: &items [one, two]\ncopy: *items\n".to_vec(),
    )
    .unwrap();
    assert_eq!(value.0["enabled"], true);
    assert_eq!(value.0["disabled"], false);
    assert_eq!(value.0["answer"], "no");
    assert_eq!(value.0["count"], 2);
    assert!(value.0["optional"].is_null());
    assert_eq!(value.0["copy"], json!(["one", "two"]));
    value.0["items"] = json!(["three"]);
    let restored: Yaml = Yaml::decode(value.clone().encode().unwrap()).unwrap();
    assert_eq!(restored, value);
    assert!(Yaml::<bool>::decode(b"yes".to_vec()).is_err());
}

#[test]
fn concrete_types_support_values_outside_the_json_data_model() {
    let expected = BTreeMap::from([(vec![1_u8, 2], "rules".to_owned())]);
    let bytes = Yaml(&expected).encode().unwrap();
    assert_eq!(
        Yaml::<BTreeMap<Vec<u8>, String>>::decode(bytes)
            .unwrap()
            .into_inner(),
        expected
    );
    let bytes = Yaml(f64::INFINITY).encode().unwrap();
    assert_eq!(Yaml::<f64>::decode(bytes).unwrap().0, f64::INFINITY);
}

#[test]
fn multiple_documents_and_duplicate_keys_are_errors() {
    for input in [
        "one\n---\ntwo\n",
        "one\n...\n---\ntwo\n",
        "name: first\nname: second\n",
        "metadata:\n  team: one\n  team: two\n",
    ] {
        let error = Yaml::<json::Value>::decode(input.as_bytes().to_vec()).unwrap_err();
        assert!(error.source().unwrap().is::<yaml::DecodeError>());
        assert!(matches!(error, Error::Decode { format: "yaml", .. }));
    }
    assert_eq!(
        Yaml::<String>::decode(b"---\nvalue\n...\n# end\n".to_vec())
            .unwrap()
            .0,
        "value"
    );
}

#[test]
fn document_end_requires_valid_trailing_content() {
    for newline in ["\n", "\r\n", "\r"] {
        for tail in [
            "@garbage",
            "invalid: [",
            "%YAML garbage",
            "---\nvalue",
            "---",
        ] {
            let input = format!("value{newline}...{newline}{tail}{newline}");
            let error = Yaml::<String>::decode(input.into_bytes()).unwrap_err();
            assert!(error.source().unwrap().is::<yaml::DecodeError>());
            assert!(matches!(error, Error::Decode { format: "yaml", .. }));
        }
        for tail in ["", " \t", "# end", " \t# end"] {
            let input = format!("value{newline}...{newline}{tail}{newline}");
            assert_eq!(
                Yaml::<String>::decode(input.into_bytes()).unwrap().0,
                "value"
            );
        }
    }
}

#[test]
fn null_documents_are_preserved_and_still_validate_the_entire_input() {
    for input in [
        "",
        "# empty\n",
        "---\n",
        "null\n",
        "~\n",
        "null\n...\n# end\n",
    ] {
        let value: Yaml = Yaml::decode(input.as_bytes().to_vec()).unwrap();
        assert!(value.0.is_null());
    }
    for input in ["null\n...\n@garbage\n", "---\n---\n", "null\n---\nnull\n"] {
        assert!(Yaml::<json::Value>::decode(input.as_bytes().to_vec()).is_err());
    }
}

#[test]
fn error_columns_count_bytes_with_unicode_line_endings_and_bom() {
    for (prefix, line, column) in [("", 1, 5), ("\r\n", 2, 5), ("\r", 2, 5), ("\u{feff}", 1, 8)] {
        let input = format!("{prefix}я: nope\n");
        let error = Yaml::<BTreeMap<String, u32>>::decode(input.into_bytes()).unwrap_err();
        assert!(error.source().unwrap().is::<yaml::DecodeError>());
        assert!(
            matches!(
                error,
                Error::Decode {
                    format: "yaml",
                    line: Some(actual_line),
                    column: Some(actual_column),
                    ..
                } if (actual_line, actual_column) == (line, column)
            ),
            "{error:?}"
        );
    }
    let error = Yaml::<json::Value>::decode("[я, }".as_bytes().to_vec()).unwrap_err();
    assert!(
        matches!(
            error,
            Error::Decode {
                format: "yaml",
                line: Some(1),
                column: Some(1),
                ..
            }
        ),
        "{error:?}"
    );
}

#[test]
fn ignored_fields_still_require_valid_utf8() {
    #[derive(Debug, Deserialize)]
    struct Empty {}
    let error = Yaml::<Empty>::decode(b"ignored: \xff\n".to_vec()).unwrap_err();
    assert!(error.source().unwrap().is::<std::str::Utf8Error>());
    assert!(matches!(
        error,
        Error::Decode {
            format: "yaml",
            line: Some(1),
            column: Some(10),
            ..
        }
    ));
}

#[test]
fn serialization_errors_keep_their_cause() {
    struct Invalid;
    impl Serialize for Invalid {
        fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
            Err(serde::ser::Error::custom("cannot encode this value"))
        }
    }
    let error = Yaml(Invalid).encode().unwrap_err();
    assert!(error.source().unwrap().is::<yaml::EncodeError>());
    assert!(matches!(error, Error::Encode { format: "yaml", .. }));
    assert_eq!(
        error.to_string(),
        "cannot encode yaml: cannot encode this value"
    );
}
