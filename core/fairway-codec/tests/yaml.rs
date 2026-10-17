//! YAML values, document boundaries, and diagnostics through the codec API.

use std::{collections::BTreeMap, error::Error as _};

use fairway_codec::{Decode, Encode, Error, Yaml, json, yaml};
use serde::Serialize;

#[test]
fn default_values_use_codec_json_types_and_strict_booleans() {
    let mut value: Yaml = Yaml::decode(
        b"name: example\nenabled: true\ndisabled: false\nanswer: no\ncount: 2\noptional: null\nitems: &items [one, two]\ncopy: *items\n".to_vec(),
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
    let expected = BTreeMap::from([(vec![1_u8, 2], "lorem".to_owned())]);
    let bytes = Yaml(&expected).encode().unwrap();
    assert_eq!(
        Yaml::<BTreeMap<Vec<u8>, String>>::decode(bytes)
            .unwrap()
            .into_inner(),
        expected
    );
    let bytes = Yaml(f64::INFINITY).encode().unwrap();
    assert_eq!(Yaml::<f64>::decode(bytes).unwrap().0, f64::INFINITY);

    let input = "# Lorem ipsum\nbody: |\n  Лорем ипсум.\n  Долор сит амет.\n";
    let record = Yaml::<BTreeMap<String, String>>::decode(input.as_bytes().to_vec()).unwrap();
    assert_eq!(record.as_ref()["body"], "Лорем ипсум.\nДолор сит амет.\n");
    let bytes = record.clone().encode().unwrap();
    assert_eq!(
        Yaml::<BTreeMap<String, String>>::decode(bytes).unwrap(),
        record
    );
}

#[test]
fn multiple_documents_and_duplicate_keys_are_errors() {
    for input in [
        "one\n---\ntwo\n",
        "one\n...\n---\ntwo\n",
        "name: first\nname: second\n",
        "extra:\n  alpha: one\n  alpha: two\n",
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
