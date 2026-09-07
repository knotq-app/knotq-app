mod support;

use knotq_mcp::protocol::{self, Routed};
use knotq_mcp::{tool_definitions, Outcome, ToolError};
use serde_json::{json, Value};
use support::Fixture;

fn route(body: &str) -> Routed {
    protocol::route(&protocol::parse(body).expect("should parse"))
}

#[test]
fn initialize_advertises_tools_and_the_protocol_revision() {
    let Routed::Respond(response) = route(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}"#,
    ) else {
        panic!("initialize must answer directly");
    };
    assert_eq!(response["id"], 1);
    assert_eq!(response["result"]["protocolVersion"], protocol::PROTOCOL_VERSION);
    assert_eq!(response["result"]["serverInfo"]["name"], "knotq");
    assert!(response["result"]["capabilities"]["tools"].is_object());
}

/// Answering a notification is a protocol violation, and some clients treat the
/// stray response as fatal.
#[test]
fn a_notification_is_answered_with_silence() {
    assert!(matches!(
        route(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#),
        Routed::Silent
    ));
    assert!(matches!(
        route(r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":1}}"#),
        Routed::Silent
    ));
}

#[test]
fn tools_list_returns_every_tool_with_an_object_schema() {
    let Routed::Respond(response) = route(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#)
    else {
        panic!("tools/list must answer directly");
    };
    let tools = response["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), tool_definitions().len());
    for tool in tools {
        assert!(tool["name"].is_string());
        assert!(
            !tool["description"].as_str().unwrap().is_empty(),
            "{} has no description",
            tool["name"]
        );
        assert_eq!(tool["inputSchema"]["type"], "object");
    }
}

#[test]
fn tools_call_is_routed_out_for_evaluation_rather_than_answered_here() {
    let Routed::CallTool {
        id,
        name,
        arguments,
    } = route(
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"list_schemes","arguments":{"include_archived":true}}}"#,
    ) else {
        panic!("tools/call must be handed to the workspace");
    };
    assert_eq!(id, json!(3));
    assert_eq!(name, "list_schemes");
    assert_eq!(arguments.unwrap()["include_archived"], json!(true));
}

#[test]
fn tools_call_without_a_name_is_an_invalid_request() {
    let Routed::Respond(response) =
        route(r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{}}"#)
    else {
        panic!("expected a direct answer");
    };
    assert_eq!(response["error"]["code"], protocol::INVALID_REQUEST);
}

#[test]
fn an_unsupported_method_is_method_not_found() {
    let Routed::Respond(response) = route(r#"{"jsonrpc":"2.0","id":5,"method":"workspace/nuke"}"#)
    else {
        panic!("expected a direct answer");
    };
    assert_eq!(response["error"]["code"], protocol::METHOD_NOT_FOUND);
}

#[test]
fn unparseable_input_produces_a_parse_error_against_a_null_id() {
    let response = protocol::parse("{not json").unwrap_err();
    assert_eq!(response["error"]["code"], protocol::PARSE_ERROR);
    assert!(response["id"].is_null());
}

#[test]
fn a_wrong_jsonrpc_version_is_rejected_but_a_missing_one_is_tolerated() {
    let Routed::Respond(response) =
        route(r#"{"jsonrpc":"1.0","id":6,"method":"tools/list"}"#)
    else {
        panic!("expected a direct answer");
    };
    assert_eq!(response["error"]["code"], protocol::INVALID_REQUEST);

    // Hand-written clients and curl one-liners routinely omit it; there is
    // nothing to gain by refusing them.
    assert!(matches!(
        route(r#"{"id":7,"method":"tools/list"}"#),
        Routed::Respond(_)
    ));
}

/// Clients ask for these even when the capability is not advertised.
#[test]
fn prompts_and_resources_are_answered_with_empty_lists() {
    for (method, key) in [
        ("prompts/list", "prompts"),
        ("resources/list", "resources"),
        ("resources/templates/list", "resourceTemplates"),
    ] {
        let body = format!(r#"{{"jsonrpc":"2.0","id":8,"method":"{method}"}}"#);
        let Routed::Respond(response) = route(&body) else {
            panic!("{method} must answer directly");
        };
        assert_eq!(response["result"][key], json!([]));
    }
}

// ── How results are wrapped ───────────────────────────────────────────────

#[test]
fn a_successful_call_carries_the_payload_twice_for_clients_that_read_either() {
    let f = Fixture::new();
    let outcome = f.call("list_schemes", json!({}));
    let response = protocol::tool_call_response(json!(1), outcome);
    let result = &response["result"];
    assert_eq!(result["isError"], json!(false));
    // Typed clients read this...
    assert!(result["structuredContent"]["schemes"].is_array());
    // ...and text-only clients read this.
    let text = result["content"][0]["text"].as_str().unwrap();
    let reparsed: Value = serde_json::from_str(text).unwrap();
    assert_eq!(reparsed["schemes"], result["structuredContent"]["schemes"]);
}

#[test]
fn a_write_is_marked_changed_and_a_no_op_is_not() {
    let f = Fixture::new();
    let item = f.item_id(f.notes, 3);

    let changed = protocol::tool_call_response(
        json!(1),
        f.call(
            "update_item",
            json!({
                "scheme_id": f.notes.to_string(),
                "item_id": item.to_string(),
                "text": "different",
            }),
        ),
    );
    assert_eq!(changed["result"]["structuredContent"]["changed"], json!(true));

    let unchanged = protocol::tool_call_response(
        json!(2),
        f.call(
            "update_item",
            json!({
                "scheme_id": f.notes.to_string(),
                "item_id": item.to_string(),
                "text": "a plain note",
            }),
        ),
    );
    assert_eq!(
        unchanged["result"]["structuredContent"]["changed"],
        json!(false)
    );
}

/// A refusal must reach the *model*, which means a successful JSON-RPC call
/// with `isError: true` — not a JSON-RPC error, which most clients swallow.
#[test]
fn a_refusal_is_a_successful_call_flagged_as_an_error() {
    let response = protocol::tool_call_response(
        json!(1),
        Err(ToolError::refused("that scheme is a linked calendar")),
    );
    assert!(response.get("error").is_none());
    assert_eq!(response["result"]["isError"], json!(true));
    assert_eq!(response["result"]["structuredContent"]["error"], "refused");
    assert!(response["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("linked calendar"));
}

#[test]
fn a_client_side_mistake_is_a_real_jsonrpc_error() {
    let response = protocol::tool_call_response(
        json!(1),
        Err(ToolError::UnknownTool("nope".into())),
    );
    assert!(response.get("result").is_none());
    assert_eq!(response["error"]["code"], protocol::METHOD_NOT_FOUND);
}

// ── The advertised surface matches the implemented one ────────────────────

#[test]
fn every_advertised_tool_is_implemented() {
    let f = Fixture::new();
    for tool in tool_definitions() {
        // Called with no arguments at all: a tool that is advertised but not
        // wired up returns `unknown_tool`, which is what this rules out. Any
        // other outcome — including a complaint about missing arguments — means
        // the tool exists.
        match f.call(tool.name, json!({})) {
            Err(e) if e.kind() == "unknown_tool" => {
                panic!("`{}` is advertised but not implemented", tool.name)
            }
            _ => {}
        }
    }
}

#[test]
fn no_tool_is_advertised_twice() {
    let mut names: Vec<&str> = tool_definitions().iter().map(|t| t.name).collect();
    let count = names.len();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), count, "duplicate tool name");
}

/// Every required argument must exist in the schema's own property list, or a
/// client that validates before sending will refuse to call the tool at all.
#[test]
fn every_required_argument_is_a_declared_property() {
    for tool in tool_definitions() {
        let properties = tool.input_schema["properties"].as_object().unwrap();
        for required in tool.input_schema["required"].as_array().unwrap() {
            let name = required.as_str().unwrap();
            assert!(
                properties.contains_key(name),
                "`{}` requires `{name}` but does not declare it",
                tool.name
            );
        }
    }
}

/// The `writes` flag is what read-only mode keys off. A write tool marked as a
/// read would stay callable with writes turned off.
#[test]
fn the_writes_flag_matches_what_each_tool_actually_does() {
    let f = Fixture::new();
    for tool in tool_definitions() {
        let refused_when_read_only = matches!(
            f.call_as(tool.name, json!({}), true),
            Err(ref e) if e.to_string().contains("read-only mode")
        );
        assert_eq!(
            refused_when_read_only, tool.writes,
            "`{}` is marked writes={} but read-only mode {} it",
            tool.name,
            tool.writes,
            if refused_when_read_only { "blocks" } else { "allows" }
        );
    }
}

/// A read tool must never produce a command; that is the property the whole
/// main-thread apply path depends on.
#[test]
fn no_read_tool_can_produce_a_command() {
    let f = Fixture::new();
    for tool in tool_definitions().iter().filter(|t| !t.writes) {
        let args = match tool.name {
            "read_scheme" => json!({ "scheme_id": f.notes.to_string() }),
            "search" => json!({ "query": "proposal" }),
            "list_calendar" => {
                json!({ "start": "2026-09-01T00:00:00Z", "end": "2026-09-30T00:00:00Z" })
            }
            _ => json!({}),
        };
        if let Ok(outcome) = f.call(tool.name, args) {
            assert!(
                matches!(outcome, Outcome::Read(_)),
                "`{}` produced {outcome:?}",
                tool.name
            );
        }
    }
}
