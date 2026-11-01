//! What the log keeps of a run that computes with a secret. Which nodes do is core's
//! (`holon_core::taint`): read off the graph before the run, by the rule `holon check`, the editor
//! and the deploy approval use too. Here: the mark the log shows in their place, and an outbound
//! request logged with the parts built from a secret withheld.

pub(crate) use holon_core::taint::{SecretFed, Taint};

/// What the log shows in place of something computed from a secret.
pub(crate) const WITHHELD: &str = "‹withheld: computed from a secret›";

/// What a logged outbound request shows when the node that made it was fed something computed
/// from a secret: the parts built from it (its path, headers or body) withheld.
pub(crate) fn withhold_request(request: &mut holon_core::Value, path: &[u32], fed: &SecretFed) {
    let holon_core::Value::Record(r) = request else { return };
    let derived = |port: usize| fed.input(path, port) == Taint::Derived;
    // A call to another flow's action: what it sends is its one input.
    if r.contains_key("flow") {
        if derived(0)
            && let Some(input) = r.get_mut("input")
        {
            *input = holon_core::Value::Text(WITHHELD.into());
        }
        return;
    }
    // An `http_request`'s inputs: its path, its headers, its body (an upload's file handle).
    if derived(0)
        && let Some(holon_core::Value::Text(url)) = r.get_mut("url")
    {
        let origin_end = url.find("://").map_or(0, |i| i + 3);
        let path_start = url[origin_end..].find('/').map_or(url.len(), |i| origin_end + i);
        url.replace_range(path_start.., &format!("/{WITHHELD}"));
    }
    if derived(1)
        && let Some(holon_core::Value::Record(headers)) = r.get_mut("headers")
    {
        for value in headers.values_mut() {
            *value = holon_core::Value::Text(WITHHELD.into());
        }
    }
    if derived(2)
        && let Some(body) = r.get_mut("body")
    {
        *body = holon_core::Value::Text(WITHHELD.into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn a_requests_derived_parts_are_withheld() {
        let fed = SecretFed::reaching(&[2], vec![Taint::Clean, Taint::Derived, Taint::Literal]);
        let text = |s: &str| holon_core::Value::Text(s.into());
        let mut request = holon_core::Value::Record(BTreeMap::from([
            ("url".to_string(), text("https://api.example.com/v1/x")),
            ("headers".to_string(), holon_core::Value::Record(BTreeMap::from([("authorization".to_string(), text("Basic c2tf"))]))),
            ("body".to_string(), text("{\"k\":\"sk_live\"}")),
        ]));
        withhold_request(&mut request, &[2], &fed);
        let holon_core::Value::Record(r) = request else { panic!() };
        assert_eq!(r["url"], text("https://api.example.com/v1/x"), "its path isn't from a secret");
        assert_eq!(r["headers"], holon_core::Value::Record(BTreeMap::from([("authorization".to_string(), text("‹withheld: computed from a secret›"))])));
        assert_eq!(r["body"], text("{\"k\":\"sk_live\"}"), "literal: redaction takes care of it");
    }
}
