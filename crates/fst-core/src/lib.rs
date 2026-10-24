pub mod journal;
pub mod methods;
pub mod store;
pub mod atlas;
pub mod mindmap;

#[cfg(test)]
mod tests {
    use crate::{journal::Journal, store::Store};

    use super::*;
    use journal::{AtomSpec, Config, Delimiters, split_atoms};
    use methods::MethodKind::*;
    use std::fs;
    use std::path::{Path, PathBuf};
    use walkdir::WalkDir;

    #[test]
    fn opens_in_wal_mode() {
        let dir = std::env::temp_dir().join("fst_test.db");
        let s = Store::open(&dir).unwrap();
        let mode: String = s
            .conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mode, "wal");
    }

    #[test]
    fn identical_content_dedupes() {
        let mut s = Store::open(":memory:").unwrap();
        s.migrate().unwrap();
        let a = s.insert_atom("x", &["idea"]).unwrap();
        let b = s.insert_atom("x", &["idea", "claim"]).unwrap();
        assert_eq!(a, b);
        let n: i64 = s
            .conn
            .query_row("SELECT count(*) FROM atoms", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn methods_registry_tracks_unique_names() {
        let mut s = Store::open(":memory:").unwrap();
        s.migrate().unwrap();
        s.insert_atom("a", &["idea", "claim"]).unwrap();
        s.insert_atom("b", &["idea"]).unwrap();
        assert_eq!(s.all_methods().unwrap(), vec!["claim", "idea"]);
    }

    #[test]
    fn bootstraps_meta_and_finds_markdown() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        fs::create_dir_all(r.join("a/b")).unwrap();
        fs::create_dir_all(r.join(".hidden")).unwrap();
        fs::write(r.join("top.md"), "").unwrap();
        fs::write(r.join("a/b/deep.MD"), "").unwrap();
        fs::write(r.join("a/notes.txt"), "").unwrap();
        fs::write(r.join(".hidden/x.md"), "").unwrap();

        let j = Journal::open(r).unwrap();

        assert!(r.join(".functorium/atoms.db").exists());
        assert!(r.join(".functorium/conf.toml").exists());
        assert_eq!(
            j.markdown_files().unwrap(),
            vec![PathBuf::from("a/b/deep.MD"), PathBuf::from("top.md")]
        );
    }

    #[test]
    fn atom_spec_forms() {
        let c: Config = toml::from_str("[atom-spec]\ndel = \"---\"").unwrap();
        assert_eq!(
            c.atom_spec.delimiters().unwrap(),
            Some(Delimiters::Single("---".into()))
        );

        let c: Config = toml::from_str("[atom-spec]\nopendel = \"<<\"\nclosedel = \">>\"").unwrap();
        assert!(matches!(
            c.atom_spec.delimiters().unwrap(),
            Some(Delimiters::Pair { .. })
        ));

        let c: Config = toml::from_str("[atom-spec]\nopendel = \"<<\"").unwrap();
        assert!(c.atom_spec.delimiters().is_err());

        let c: Config = toml::from_str("").unwrap();
        assert_eq!(c.atom_spec.delimiters().unwrap(), None);
    }

    #[test]
    fn single_splits_between_and_to_eof() {
        let d = Delimiters::Single("---".into());
        assert_eq!(
            split_atoms("a\n---\nb\n---\nc\n", Some(&d)),
            vec!["a", "b", "c"]
        );
    }

    fn single() -> Delimiters {
        Delimiters::Single("---".into())
    }
    fn pair() -> Delimiters {
        Delimiters::Pair {
            open: "<<".into(),
            close: ">>".into(),
        }
    }

    #[test]
    fn single_cases() {
        let s = single();
        assert_eq!(
            split_atoms("a\n---\nb\n---\nc\n", Some(&s)),
            vec!["a", "b", "c"]
        );
        assert_eq!(split_atoms("---\na\n---\nb", Some(&s)), vec!["a", "b"]);
        assert_eq!(split_atoms("a\n---", Some(&s)), vec!["a"]);
        assert_eq!(split_atoms("a\n---\n---\nb", Some(&s)), vec!["a", "b"]);
        assert_eq!(split_atoms("a\r\n---\r\nb", Some(&s)), vec!["a", "b"]);
        assert_eq!(
            split_atoms("a --- b\n---\nc", Some(&s)),
            vec!["a --- b", "c"]
        );
    }

    #[test]
    fn pair_cases() {
        let p = pair();
        assert_eq!(
            split_atoms("<<\na\n>>\n<<\nb\n>>", Some(&p)),
            vec!["a", "b"]
        );
        assert_eq!(
            split_atoms("x\n<<\na\n>>\n\n<<\nb\n>>\n", Some(&p)),
            vec!["a", "b"]
        );
        assert_eq!(split_atoms("<<\r\na\r\n>>\r\n", Some(&p)), vec!["a"]);
        assert_eq!(split_atoms("<<\na\n>>\n<<\nb", Some(&p)), vec!["a"]); // unclosed dropped
        let same = Delimiters::Pair {
            open: "$$".into(),
            close: "$$".into(),
        };
        assert_eq!(
            split_atoms("t\n$$\na\n$$\nu\n$$\nb\n$$\n", Some(&same)),
            vec!["a", "b"]
        );
    }

    #[test]
    fn no_delimiters_is_one_atom() {
        assert_eq!(split_atoms("  whole  \n", None), vec!["whole"]);
    }
    fn one(s: &str) -> (String, methods::MethodKind) {
        let v = methods::extract_methods(s);
        assert_eq!(v.len(), 1, "{s:?} -> {v:?}");
        (v[0].text.clone(), v[0].kind)
    }

    #[test]
    fn the_six_forms() {
        assert_eq!(one("#physics"), ("physics".into(), Plain));
        assert_eq!(
            one("#physics/cosmology"),
            ("physics/cosmology".into(), Ordered)
        );
        assert_eq!(one("#a--b--c"), ("a--b--c".into(), Unordered));
        assert_eq!(one("#f/name"), ("f/name".into(), Functional));
        assert_eq!(one("#f/name/x/y"), ("f/name/x/y".into(), FunctionalOrdered));
        assert_eq!(
            one("#f/name--x--y"),
            ("f/name--x--y".into(), FunctionalUnordered)
        );
    }

    #[test]
    fn hyphens_and_unicode() {
        assert_eq!(one("#some-name"), ("some-name".into(), Plain));
        assert_eq!(one("#a-b--c-d"), ("a-b--c-d".into(), Unordered));
        assert_eq!(one("#f/some-name"), ("f/some-name".into(), Functional));
        assert_eq!(one("#физика"), ("физика".into(), Plain));
    }

    #[test]
    fn f_is_only_special_with_slash() {
        assert_eq!(one("#f"), ("f".into(), Plain));
        assert_eq!(one("#f--a"), ("f--a".into(), Unordered));
    }

    #[test]
    fn headings_are_not_methods() {
        assert!(methods::extract_methods("# Title\n## Sub\n### Deeper").is_empty());
    }

    #[test]
    fn trailing_punctuation_and_dedupe() {
        let v = methods::extract_methods("see #physics. and #a/b, then #physics again");
        let t: Vec<_> = v.iter().map(|m| m.text.as_str()).collect();
        assert_eq!(t, vec!["physics", "a/b"]);
    }

    #[test]
    fn invalid_or_embedded_tokens_are_ignored() {
        assert!(methods::extract_methods("#a/b--c").is_empty());
        assert!(methods::extract_methods("http://x.org/p#frag").is_empty());
        assert!(methods::extract_methods("C#").is_empty());
        assert!(methods::extract_methods("#a//b #/a #a/ #").is_empty());
    }

    #[test]
    fn start_of_text_and_newlines() {
        assert_eq!(methods::extract_methods("#a\n#b/c\ntext #d").len(), 3);
    }

    fn texts(s: &str) -> Vec<String> {
        methods::extract_methods(s)
            .into_iter()
            .map(|m| m.text)
            .collect()
    }

    #[test]
    fn fenced_blocks_are_skipped() {
        assert_eq!(texts("text #a\n```\n#b\n```\n#c"), vec!["a", "c"]);
        assert_eq!(texts("```rust\n#x\n```\n#y"), vec!["y"]);
        assert_eq!(texts("#a\n```\nx\n```\n#b"), vec!["a", "b"]);
    }

    #[test]
    fn unclosed_fence_runs_to_end() {
        assert_eq!(texts("#a\n```\n#b"), vec!["a"]);
    }

    #[test]
    fn fence_closes_only_on_bare_fence() {
        assert_eq!(texts("```\n```not closing\n#x\n```\n#y"), vec!["y"]);
    }

    #[test]
    fn crlf_fences() {
        assert_eq!(texts("#a\r\n```\r\n#b\r\n```\r\n#c"), vec!["a", "c"]);
    }

    #[test]
    fn indented_four_is_not_a_fence() {
        assert_eq!(texts("    ```\n#b"), vec!["b"]);
    }

    #[test]
    fn inline_code_is_skipped() {
        assert_eq!(texts("use `#b` and #c"), vec!["c"]);
        assert_eq!(texts("`#b`#c"), Vec::<String>::new());
        assert_eq!(texts("a ` #b"), vec!["b"]); // unclosed backtick is literal
        assert_eq!(texts("```code``` #b"), vec!["b"]); // not a fence
    }
}
