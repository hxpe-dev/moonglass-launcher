//! Emulator launch templates (spec section 2, "Launch Template Syntax").
//!
//! A template holds arguments only. It is split into an argument vector
//! *before* placeholders are substituted, so a path containing spaces,
//! quotes or shell metacharacters always stays one argument and is never
//! interpreted. Nothing here goes through a shell.
//!
//! Quoting: `"..."` and `'...'` group words into one argument and are
//! removed. There are no backslash escapes, because Windows paths in a
//! template (for example a fixed config path) use backslashes literally.

use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TemplateError {
    #[error("unterminated {0} quote in launch template")]
    UnterminatedQuote(char),
    #[error("unknown placeholder {{{0}}}; expected {{rom}}, {{rom_dir}} or {{rom_name}}")]
    UnknownPlaceholder(String),
    #[error("unclosed '{{' in launch template")]
    UnclosedBrace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Placeholder {
    Rom,
    RomDir,
    RomName,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Piece {
    Text(String),
    Var(Placeholder),
}

/// A validated template, parsed once and expanded per launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Template {
    args: Vec<Vec<Piece>>,
}

/// Values substituted into a template.
#[derive(Debug, Clone)]
pub struct RomVars<'a> {
    /// Full path to the ROM file or game folder.
    pub rom: &'a str,
    /// Directory containing the ROM.
    pub rom_dir: &'a str,
    /// File name without extension.
    pub rom_name: &'a str,
}

impl Template {
    pub fn parse(src: &str) -> Result<Self, TemplateError> {
        let mut args = Vec::new();
        for word in split_words(src)? {
            args.push(parse_placeholders(&word)?);
        }
        Ok(Self { args })
    }

    pub fn expand(&self, vars: &RomVars<'_>) -> Vec<String> {
        self.args
            .iter()
            .map(|pieces| {
                let mut s = String::new();
                for p in pieces {
                    match p {
                        Piece::Text(t) => s.push_str(t),
                        Piece::Var(Placeholder::Rom) => s.push_str(vars.rom),
                        Piece::Var(Placeholder::RomDir) => s.push_str(vars.rom_dir),
                        Piece::Var(Placeholder::RomName) => s.push_str(vars.rom_name),
                    }
                }
                s
            })
            .collect()
    }
}

/// Splits per-game launch arguments with the same quoting rules, for
/// executables, which have no placeholders.
pub fn split_args(src: &str) -> Result<Vec<String>, TemplateError> {
    split_words(src)
}

/// Splits on unquoted whitespace. `""` produces an empty argument.
fn split_words(src: &str) -> Result<Vec<String>, TemplateError> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut quote: Option<char> = None;
    for c in src.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => cur.push(c),
            None if c == '"' || c == '\'' => {
                quote = Some(c);
                in_word = true;
            }
            None if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            None => {
                cur.push(c);
                in_word = true;
            }
        }
    }
    if let Some(q) = quote {
        return Err(TemplateError::UnterminatedQuote(q));
    }
    if in_word {
        words.push(cur);
    }
    Ok(words)
}

fn parse_placeholders(word: &str) -> Result<Vec<Piece>, TemplateError> {
    let mut pieces = Vec::new();
    let mut rest = word;
    while let Some(start) = rest.find('{') {
        if start > 0 {
            pieces.push(Piece::Text(rest[..start].to_string()));
        }
        let after = &rest[start + 1..];
        let end = after.find('}').ok_or(TemplateError::UnclosedBrace)?;
        let name = &after[..end];
        let var = match name {
            "rom" => Placeholder::Rom,
            "rom_dir" => Placeholder::RomDir,
            "rom_name" => Placeholder::RomName,
            other => return Err(TemplateError::UnknownPlaceholder(other.to_string())),
        };
        pieces.push(Piece::Var(var));
        rest = &after[end + 1..];
    }
    if !rest.is_empty() {
        pieces.push(Piece::Text(rest.to_string()));
    }
    Ok(pieces)
}

#[cfg(test)]
mod tests {
    use super::*;

    const VARS: RomVars<'static> = RomVars {
        rom: r#"C:\Games\GC\Zelda "Wind" Waker; rm -rf.rvz"#,
        rom_dir: r"C:\Games\GC",
        rom_name: r#"Zelda "Wind" Waker; rm -rf"#,
    };

    fn expand(t: &str) -> Vec<String> {
        Template::parse(t).unwrap().expand(&VARS)
    }

    #[test]
    fn dolphin_example() {
        assert_eq!(expand(r#"-b -e "{rom}""#), ["-b", "-e", VARS.rom]);
    }

    #[test]
    fn unquoted_placeholder_is_still_one_argument() {
        assert_eq!(expand("-e {rom}"), ["-e", VARS.rom]);
    }

    #[test]
    fn placeholders_inside_words() {
        assert_eq!(
            expand("--save={rom_dir}/saves/{rom_name}.sav"),
            [format!(
                "--save={}/saves/{}.sav",
                VARS.rom_dir, VARS.rom_name
            )]
        );
    }

    #[test]
    fn flatpak_style_template_and_quotes() {
        assert_eq!(
            expand(r#"run org.DolphinEmu.dolphin-emu -b 'a b' "" -e {rom}"#),
            [
                "run",
                "org.DolphinEmu.dolphin-emu",
                "-b",
                "a b",
                "",
                "-e",
                VARS.rom
            ]
        );
        assert_eq!(
            expand(r"-L C:\cores\x.dll {rom}"),
            ["-L", r"C:\cores\x.dll", VARS.rom]
        );
        assert!(expand("   ").is_empty());
    }

    #[test]
    fn errors() {
        assert_eq!(
            Template::parse(r#"-e "{rom}"#),
            Err(TemplateError::UnterminatedQuote('"'))
        );
        assert_eq!(
            Template::parse("{path}"),
            Err(TemplateError::UnknownPlaceholder("path".into()))
        );
        assert_eq!(Template::parse("{rom"), Err(TemplateError::UnclosedBrace));
    }
}
