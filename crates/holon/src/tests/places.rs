//! Whether an answer names a place on the server's disk, for every test that holds one doesn't
//! (R127). Looked for in the text as sent, a Windows path is never found in a JSON answer: JSON
//! escapes its backslashes, so `!said.contains(path)` holds whatever the answer says, and a check
//! written that way can't fail on Windows (it fails on Linux alone, where a path has none). Here
//! a place is looked for in every string read out of the answer, and in the text as sent, each as
//! `{}` writes it, as `{:?}` does (its backslashes doubled in the words themselves) and with its
//! backslashes as forward slashes (as a file URL or a path made uniform writes it).
//! tests.rs's `no_test_looks_for_a_place_in_an_answers_text` holds every such check to this
//! file. The server's integration tests take it by its path, as they take source.rs.

/// Every string `said` holds, its keys too, read out of its JSON, with `leave` taken off its top
/// first; the text itself if it isn't JSON.
pub(crate) fn strings_of(said: &str, leave: &[&str]) -> Vec<String> {
    fn walk(v: &serde_json::Value, out: &mut Vec<String>) {
        match v {
            serde_json::Value::String(s) => out.push(s.clone()),
            serde_json::Value::Array(items) => items.iter().for_each(|x| walk(x, out)),
            serde_json::Value::Object(fields) => fields.iter().for_each(|(k, x)| {
                out.push(k.clone());
                walk(x, out)
            }),
            _ => {}
        }
    }
    let Ok(mut v) = serde_json::from_str::<serde_json::Value>(said) else { return vec![said.to_string()] };
    if let Some(fields) = v.as_object_mut() {
        for key in leave {
            fields.remove(*key);
        }
    }
    let mut out = Vec::new();
    walk(&v, &mut out);
    out
}

/// Each of `places` that `said` tells, with `leave` taken off the top of its JSON: looked for in
/// each string read out of it ([`strings_of`]), and in the text as sent unless something is left
/// out, each as `{}` writes it, as `{:?}` does and with forward slashes. No places, or an empty
/// one, is refused: an empty place is in every text, and none is nothing looked for, so a check
/// handed either could hold nothing.
#[track_caller]
pub(crate) fn places_told(said: &str, leave: &[&str], places: &[&str]) -> Vec<String> {
    assert!(!places.is_empty() && !places.contains(&""), "no place to look for in {places:?}: an empty one is in every text, and none is nothing looked for");
    let mut texts = strings_of(said, leave);
    if leave.is_empty() {
        texts.push(said.to_string());
    }
    places
        .iter()
        .filter(|place| {
            let forms = [place.to_string(), place.replace('\\', "\\\\"), place.replace('\\', "/")];
            texts.iter().any(|t| forms.iter().any(|form| t.contains(form.as_str())))
        })
        .map(|place| place.to_string())
        .collect()
}

/// `said` names none of `places` ([`places_told`]): `what`, and the answer, if it does.
#[track_caller]
pub(crate) fn names_no_place(what: &str, said: &str, leave: &[&str], places: &[&str]) {
    let told = places_told(said, leave, places);
    assert!(told.is_empty(), "{what}: names a place on the server's disk ({told:?}): {said}");
}

/// The system's temporary folder as `{}` writes it, without a separator at its end: every test's
/// data folder is under it.
pub(crate) fn temp_folder() -> String {
    std::env::temp_dir().display().to_string().trim_end_matches(['/', '\\']).to_string()
}
