//! The sea-battle sample, played through the real server: two captains seal their fleets, fire
//! and answer to the end, and reveal. An honest game goes to who sank the fleet; a captain who
//! answered "miss" to a hit, or revealed another board than the one sealed, loses whatever the
//! score, with the squares lied about named. No key is ever in what anyone is shown, and holon
//! verify replays every run of the games.

use super::*;
use serde_json::{json, Value};

/// Two fleets of ships 4, 3, 3 and 2 in straight lines; row by row, "0" water.
const ADA: &str = "1111000000000000222000000000000000003000000030000000300044000000";
const MERT: &str = "0000000010000000100000001000000010000000000222003330000000000044";

struct Captain {
    board: &'static str,
    salt: &'static str,
    key: String,
}

impl Captain {
    fn seal(&self) -> String {
        crate::sigv4::sha256_hex(format!("{}|{}", self.board, self.salt).as_bytes())
    }
}

fn send(state: &AppState, method: &str, url: &str, body: Value) -> (u16, Value) {
    let request = data::DataRequest { headers: vec![("content-type".into(), "application/json".into())], ..fake_data(method, url, body.to_string().into_bytes()) };
    let answer = serve_data(&state.store, &state.deployments, &state.deployments_path, "sea-battle", &request);
    (answer.status, serde_json::from_slice(&answer.body).unwrap_or(Value::Null))
}

/// The sample deployed, with what it asks for approved.
fn deployed() -> AppState {
    let state = test_state();
    let plan = holon_author::agent::plan(holon_author::samples::sample("sea-battle").unwrap().text).unwrap();
    for u in &plan.units {
        assert_eq!(route(&state, &req("POST", "/unit", u.clone())).status_code, 200);
    }
    let preview = body_json(route(&state, &req("POST", "/deploy/preview", serde_json::to_vec(&json!({ "slug": plan.name, "unit": plan.unit })).unwrap())));
    let r = deploy_json(&state, &plan.name, parse_hash(&plan.unit).unwrap(), json!({ "acknowledge": preview["needs_approval"], "document": plan.document }));
    assert_eq!(r.status_code, 200);
    state
}

/// A game played to its end: each captain fires at the other's squares in order, 0 to 63, and
/// answers truly but for the squares in its `lies`, where it says "miss"; then each reveals the
/// board given (its own, unless `moved`). Answers the game as anyone sees it at the end.
fn play(state: &AppState, lies: [&[u64]; 2], moved: [Option<&'static str>; 2]) -> Value {
    let mut c = [Captain { board: ADA, salt: "salt-of-ada-0001", key: String::new() }, Captain { board: MERT, salt: "salt-of-mert-002", key: String::new() }];
    let (code, made) = send(state, "POST", "/games", json!({ "name": "Ada", "commit": c[0].seal() }));
    assert_eq!(code, 201, "{made}");
    let game = made["game"].as_str().unwrap().to_string();
    c[0].key = made["key"].as_str().unwrap().into();
    let (code, joined) = send(state, "POST", &format!("/games/{game}/join"), json!({ "name": "Mert", "commit": c[1].seal() }));
    assert_eq!(code, 200, "{joined}");
    c[1].key = joined["key"].as_str().unwrap().into();
    assert_eq!(send(state, "POST", &format!("/games/{game}/join"), json!({ "name": "Eve", "commit": c[1].seal() })).0, 409, "a third captain is turned away");
    assert_eq!(send(state, "POST", &format!("/games/{game}/fire"), json!({ "key": c[1].key, "cell": 0 })).0, 409, "Ada fires first");
    assert_eq!(send(state, "POST", &format!("/games/{game}/fire"), json!({ "key": "not a key", "cell": 0 })).0, 409, "a stranger fires never");

    let mut next = [0u64, 0u64];
    let mut turn = 0;
    let mut seen = Value::Null;
    for _ in 0..128 {
        let other = 1 - turn;
        let cell = next[turn];
        next[turn] += 1;
        let (code, fired) = send(state, "POST", &format!("/games/{game}/fire"), json!({ "key": c[turn].key, "cell": cell }));
        assert_eq!(code, 200, "{fired}");
        assert_eq!(send(state, "POST", &format!("/games/{game}/fire"), json!({ "key": c[turn].key, "cell": cell + 1 })).0, 409, "a shot waits for its answer");
        let hit = c[other].board.as_bytes()[cell as usize] != b'0' && !lies[other].contains(&cell);
        let (code, answered) = send(state, "POST", &format!("/games/{game}/answer"), json!({ "key": c[other].key, "said": if hit { "hit" } else { "miss" } }));
        assert_eq!(code, 200, "{answered}");
        seen = answered;
        if seen["phase"] != "play" {
            break;
        }
        turn = other;
    }
    assert_eq!(seen["phase"], "reveal");
    for (i, captain) in c.iter().enumerate() {
        let board = moved[i].unwrap_or(captain.board);
        let (code, shown) = send(state, "POST", &format!("/games/{game}/reveal"), json!({ "key": captain.key, "board": board, "salt": captain.salt }));
        assert_eq!(code, 200, "{shown}");
    }
    let (code, end) = send(state, "GET", &format!("/games/{game}"), Value::Null);
    assert_eq!(code, 200);
    assert_eq!(end["phase"], "over");
    assert!(end["a"].get("key").is_none() && end["b"].get("key").is_none(), "no key is shown: {end}");
    end
}

#[test]
fn the_sea_battle_sample_catches_a_captain_who_lies_or_moves_a_ship() {
    let state = deployed();

    // Both honest: Mert's twelfth hit is his 58th shot (square 57), before Ada's (square 63).
    let fair = play(&state, [&[], &[]], [None, None]);
    assert_eq!(fair["winner"], "b", "{fair}");
    assert_eq!(fair["why"], "Mert sank the whole fleet, and both boards matched their seals and every answer.");
    assert_eq!((fair["a"]["honest"].clone(), fair["b"]["honest"].clone()), (json!(true), json!(true)));

    // Mert says "miss" to two of Ada's hits (squares 8 and 16 hold his ship): Ada wins, those two
    // named.
    let lied = play(&state, [&[], &[8, 16]], [None, None]);
    assert_eq!(lied["winner"], "a", "{lied}");
    assert_eq!(lied["b"]["lies"], json!([8, 16]));
    assert_eq!(lied["why"], "Mert lied about 2 shots, so Ada wins.");

    // Ada reveals her fleet moved one square on: not the board she sealed, and Mert wins.
    let shifted: &'static str = Box::leak(format!("0{}", &ADA[..63]).into_boxed_str());
    let moved = play(&state, [&[], &[]], [Some(shifted), None]);
    assert_eq!(moved["winner"], "b", "{moved}");
    assert_eq!(moved["a"]["sealed"], false);
    assert_eq!(moved["why"], "Ada revealed a board that is not the one sealed before the first shot, so Mert wins.");

    // Every run of the three games replays as its log says.
    let export: verify::Export = serde_json::from_value(body_json(route(&state, &req("GET", "/deployments/sea-battle/export", vec![])))).unwrap();
    let report = verify::check(&export, None, &BTreeMap::new());
    assert!(report.ok() && report.reruns.iter().all(|(_, r)| *r == verify::Rerun::Matched), "{}", verify::describe(&report, None));
}
