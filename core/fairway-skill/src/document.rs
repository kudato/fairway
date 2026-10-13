//! The text of skills and references: frontmatter, body, and their checks.
//!
//! A skill file starts with frontmatter: a `---` line, a YAML mapping with
//! only a nonempty `description`, and another `---` line. The rest of the file
//! is the body, a template. A reference file is a template as a whole and
//! has no frontmatter.

use std::iter;

use fairway_codec::{Decode, Yaml, serde::Deserialize};
use fairway_template::Template;

use crate::address::Kind;

/// A skill file split into its frontmatter and body.
#[derive(Debug)]
pub(crate) struct Skill<'a> {
    /// The frontmatter from its opening `---` line, which YAML reads as the
    /// start of a document; YAML positions are therefore those of the file.
    frontmatter: &'a str,
    body: &'a str,
    /// The number of lines before the body.
    lines: usize,
}

#[derive(Deserialize)]
#[serde(crate = "fairway_codec::serde", deny_unknown_fields)]
struct Frontmatter {
    description: String,
}

/// Splits a skill file and returns the reason it has no frontmatter.
pub(crate) fn split(text: &str) -> Result<Skill<'_>, &'static str> {
    const MISSING: &str = "has no frontmatter: a skill starts with a `---` line";
    let mut end = 0;
    for (index, line) in text.split_inclusive('\n').enumerate() {
        let start = end;
        end += line.len();
        if index == 0 {
            if !delimiter(line) {
                return Err(MISSING);
            }
        } else if delimiter(line) {
            return Ok(Skill {
                frontmatter: &text[..start],
                body: &text[end..],
                lines: index + 1,
            });
        }
    }
    Err(if text.is_empty() {
        MISSING
    } else {
        "has no end of frontmatter: a `---` line after it"
    })
}

/// Whether a line, with its line ending, is a frontmatter delimiter.
fn delimiter(line: &str) -> bool {
    line.strip_suffix('\n')
        .map_or(line, |line| line.strip_suffix('\r').unwrap_or(line))
        == "---"
}

impl Skill<'_> {
    /// Reads the description and returns the reason the frontmatter is invalid.
    pub(crate) fn description(&self) -> Result<String, String> {
        let Yaml(Frontmatter { description }) = Yaml::decode(self.frontmatter.as_bytes().to_vec())
            .map_err(|error| format!("has invalid frontmatter: {error}"))?;
        if description.trim().is_empty() {
            return Err("has an empty description".to_owned());
        }
        Ok(description)
    }

    /// Compiles the body; template positions are those of the file.
    pub(crate) fn template(&self, name: &str) -> Result<Template, fairway_template::Error> {
        // The comment strips the padding before it, so the body is rendered
        // as it is, but starts on its own line of the file.
        let comment = "{#- #}";
        let mut source = String::with_capacity(self.lines + comment.len() + self.body.len());
        source.extend(iter::repeat_n('\n', self.lines));
        source.push_str(comment);
        source.push_str(self.body);
        Template::compile(name, source)
    }
}

/// Checks the text for an address of `kind` and returns every problem, each
/// phrased to follow the address and the file in a message.
///
/// `name` is a valid address: it names the template in messages.
pub(crate) fn check(name: &str, kind: Kind, text: &str) -> Vec<String> {
    let mut problems = Vec::new();
    match kind {
        Kind::Skill => match split(text) {
            Ok(skill) => {
                if let Err(reason) = skill.description() {
                    problems.push(reason);
                }
                if let Err(error) = skill.template(name) {
                    problems.push(error.to_string());
                }
            }
            Err(reason) => problems.push(reason.to_owned()),
        },
        Kind::Reference => {
            if split(text).is_ok() {
                problems.push("starts with frontmatter, which a reference cannot have".to_owned());
            }
            if let Err(error) = Template::compile(name, text) {
                problems.push(error.to_string());
            }
        }
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::{Kind, check, split};

    const ADDRESS: &str = "skill/writing/technical";

    fn problems(kind: Kind, text: &str) -> Vec<String> {
        check(ADDRESS, kind, text)
    }

    #[test]
    fn a_skill_is_split_into_frontmatter_and_body() {
        let skill = split("---\ndescription: Write docs.\n---\n# Body\n").unwrap();
        assert_eq!(skill.frontmatter, "---\ndescription: Write docs.\n");
        assert_eq!(skill.body, "# Body\n");
        assert_eq!(skill.lines, 3);
        assert_eq!(skill.description().unwrap(), "Write docs.");

        let skill = split("---\r\ndescription: x\r\n---\r\n\r\nBody").unwrap();
        assert_eq!(skill.body, "\r\nBody");
        assert_eq!(skill.description().unwrap(), "x");

        let skill = split("---\ndescription: x\n---").unwrap();
        assert_eq!(skill.body, "");
        assert_eq!(skill.lines, 3);
    }

    #[test]
    fn the_body_renders_as_written_and_keeps_file_lines() {
        let text = "---\ndescription: x\n---\n\n  Hello, {{ name }}!\n";
        let template = split(text).unwrap().template(ADDRESS).unwrap();
        assert_eq!(
            template
                .render(std::collections::BTreeMap::from([("name", "Ann")]))
                .unwrap(),
            "\n  Hello, Ann!\n"
        );
        let error = template.render(()).unwrap_err();
        assert_eq!(error.line(), Some(5), "{error}");

        let text = "---\ndescription: x\n---\nline 4\n{% if %}\n";
        let Err(error) = split(text).unwrap().template(ADDRESS) else {
            panic!("the template compiles");
        };
        assert_eq!(error.line(), Some(5), "{error}");
    }

    #[test]
    fn a_skill_needs_frontmatter_with_only_a_nonempty_description() {
        assert_eq!(
            problems(Kind::Skill, "# Body\n"),
            ["has no frontmatter: a skill starts with a `---` line"]
        );
        assert_eq!(
            problems(Kind::Skill, " ---\ndescription: x\n---\n"),
            ["has no frontmatter: a skill starts with a `---` line"]
        );
        assert_eq!(
            problems(Kind::Skill, "---\ndescription: x\n"),
            ["has no end of frontmatter: a `---` line after it"]
        );
        for (text, expected) in [
            (
                "---\ndescription: \"  \"\n---\n",
                "has an empty description",
            ),
            ("---\ndescription: \"\"\n---\n", "has an empty description"),
            ("---\n---\n", "has invalid frontmatter: "),
            ("---\nname: technical\n---\n", "has invalid frontmatter: "),
            (
                "---\nname: technical\ndescription: x\n---\n",
                "has invalid frontmatter: ",
            ),
            ("---\ndescription: [x]\n---\n", "has invalid frontmatter: "),
        ] {
            let problems = problems(Kind::Skill, text);
            assert_eq!(problems.len(), 1, "{text:?}: {problems:?}");
            assert!(problems[0].starts_with(expected), "{text:?}: {problems:?}");
        }
        assert!(problems(Kind::Skill, "---\ndescription: x\n---\nBody {{ x }}\n").is_empty());
    }

    #[test]
    fn every_problem_of_a_skill_is_reported() {
        let problems = problems(Kind::Skill, "---\ndescription: \" \"\n---\n{{ \n");
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert_eq!(problems[0], "has an empty description");
        assert!(
            problems[1].starts_with("skill/writing/technical:4: cannot compile template: "),
            "{problems:?}"
        );
    }

    #[test]
    fn a_reference_is_a_template_without_frontmatter() {
        assert!(problems(Kind::Reference, "# STE\n\n{{ x }}\n").is_empty());
        assert!(problems(Kind::Reference, "---\n\nA thematic break.\n").is_empty());
        assert_eq!(
            problems(Kind::Reference, "---\ndescription: x\n---\nText\n"),
            ["starts with frontmatter, which a reference cannot have"]
        );
        let problems = problems(Kind::Reference, "Text\n{% endif %}\n");
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(
            problems[0].starts_with("skill/writing/technical:2: cannot compile template: "),
            "{problems:?}"
        );
    }
}
