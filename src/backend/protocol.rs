//! Pure builders for GUI -> sidecar protocol ops.
//!
//! Each function returns the JSON `Value` envelope for one op. Keeping the
//! encoding separate from the transport ([`super::CodePuppy::write`]) means
//! every op shape is unit-testable without spawning a process — this is the
//! Rust side of the Rust<->Python protocol contract.

use serde_json::{Value, json};

use super::{AgentConfigDraft, AskAnswer, JudgeDraft};

pub(super) fn prompt(id: u64, text: &str, images: &[String]) -> Value {
    let mut op = json!({ "op": "prompt", "id": id, "text": text });
    // Only carry `images` when there are some, so plain text prompts stay tidy.
    if !images.is_empty() {
        op["images"] = json!(images);
    }
    op
}

pub(super) fn cancel() -> Value {
    json!({ "op": "cancel" })
}

pub(super) fn command(id: u64, text: &str) -> Value {
    json!({ "op": "command", "id": id, "text": text })
}

pub(super) fn list_commands() -> Value {
    json!({ "op": "list_commands" })
}

pub(super) fn list_agents() -> Value {
    json!({ "op": "list_agents" })
}

pub(super) fn list_models() -> Value {
    json!({ "op": "list_models" })
}

pub(super) fn set_model(name: &str) -> Value {
    json!({ "op": "set_model", "name": name })
}

pub(super) fn status() -> Value {
    json!({ "op": "status" })
}

pub(super) fn list_sessions() -> Value {
    json!({ "op": "list_sessions" })
}

pub(super) fn load_session(name: &str, source: &str) -> Value {
    json!({ "op": "load_session", "name": name, "source": source })
}

pub(super) fn preview_session(name: &str, source: &str) -> Value {
    json!({ "op": "preview_session", "name": name, "source": source })
}

pub(super) fn pause() -> Value {
    json!({ "op": "pause" })
}

pub(super) fn resume() -> Value {
    json!({ "op": "resume" })
}

pub(super) fn steer(text: &str, mode: &str) -> Value {
    json!({ "op": "steer", "text": text, "mode": mode })
}

pub(super) fn complete(id: u64, text: &str, cursor: usize) -> Value {
    json!({ "op": "complete", "id": id, "text": text, "cursor": cursor })
}

pub(super) fn ask_response(id: &str, answers: &[AskAnswer]) -> Value {
    let answers = serde_json::to_value(answers).unwrap_or(Value::Array(vec![]));
    json!({ "op": "ask_response", "id": id, "cancelled": false, "answers": answers })
}

pub(super) fn ask_cancel(id: &str) -> Value {
    json!({ "op": "ask_response", "id": id, "cancelled": true })
}

pub(super) fn respond_input(prompt_id: &str, value: &str) -> Value {
    json!({ "op": "respond_input", "prompt_id": prompt_id, "value": value })
}

pub(super) fn respond_confirmation(
    prompt_id: &str,
    confirmed: bool,
    feedback: Option<&str>,
) -> Value {
    json!({
        "op": "respond_confirmation",
        "prompt_id": prompt_id,
        "confirmed": confirmed,
        "feedback": feedback,
    })
}

pub(super) fn respond_selection(prompt_id: &str, index: i64, value: &str) -> Value {
    json!({
        "op": "respond_selection",
        "prompt_id": prompt_id,
        "selected_index": index,
        "selected_value": value,
    })
}

pub(super) fn set_agent(name: &str) -> Value {
    json!({ "op": "set_agent", "name": name })
}

pub(super) fn shutdown() -> Value {
    json!({ "op": "shutdown" })
}

pub(super) fn list_mcp_servers() -> Value {
    json!({ "op": "list_mcp_servers" })
}

pub(super) fn set_mcp_enabled(name: &str, enabled: bool) -> Value {
    json!({ "op": "set_mcp_enabled", "name": name, "enabled": enabled })
}

/// `transport` is "stdio" | "sse" | "http"; `config` carries the transport's
/// fields (command/args/env or url/headers) exactly as Code Puppy stores them.
pub(super) fn add_mcp_server(name: &str, transport: &str, config: &Value) -> Value {
    json!({ "op": "add_mcp_server", "name": name, "type": transport, "config": config })
}

pub(super) fn list_skills() -> Value {
    json!({ "op": "list_skills" })
}

pub(super) fn get_skill(name: &str) -> Value {
    json!({ "op": "get_skill", "name": name })
}

pub(super) fn set_skill_enabled(name: &str, enabled: bool) -> Value {
    json!({ "op": "set_skill_enabled", "name": name, "enabled": enabled })
}

/// `content` is the markdown body (the sidecar wraps it in name/description
/// frontmatter); `scope` is "user" (global) or "project" (this folder).
pub(super) fn save_skill(name: &str, description: &str, content: &str, scope: &str) -> Value {
    json!({
        "op": "save_skill",
        "name": name,
        "description": description,
        "content": content,
        "scope": scope,
    })
}

pub(super) fn list_agent_configs() -> Value {
    json!({ "op": "list_agent_configs" })
}

pub(super) fn get_agent_config(name: &str) -> Value {
    json!({ "op": "get_agent_config", "name": name })
}

/// The full agent draft from the visual builder. Empty `model`/`user_prompt`
/// are still sent; the sidecar omits them from the on-disk JSON.
pub(super) fn save_agent_config(d: &AgentConfigDraft) -> Value {
    json!({
        "op": "save_agent_config",
        "name": d.name,
        "display_name": d.display_name,
        "description": d.description,
        "system_prompt": d.system_prompt,
        "user_prompt": d.user_prompt,
        "model": d.model,
        "tools": d.tools,
        "mcp_servers": d.mcp_servers,
        "scope": d.scope,
    })
}

pub(super) fn delete_agent_config(name: &str) -> Value {
    json!({ "op": "delete_agent_config", "name": name })
}

pub(super) fn clone_agent_config(name: &str) -> Value {
    json!({ "op": "clone_agent_config", "name": name })
}

// --- Puppy Kennel (read-only memory browser) ------------------------------

pub(super) fn kennel_stats() -> Value {
    json!({ "op": "kennel_stats" })
}

pub(super) fn kennel_list_wings() -> Value {
    json!({ "op": "kennel_list_wings" })
}

/// `wings` empty -> all wings. `limit` bounds the render tail.
pub(super) fn kennel_recent(wings: &[String], limit: u64) -> Value {
    let mut op = json!({ "op": "kennel_recent", "limit": limit });
    if !wings.is_empty() {
        op["wings"] = json!(wings);
    }
    op
}

/// FTS5 BM25 search; `wings` empty -> all wings.
pub(super) fn kennel_search(query: &str, wings: &[String], limit: u64) -> Value {
    let mut op = json!({ "op": "kennel_search", "query": query, "limit": limit });
    if !wings.is_empty() {
        op["wings"] = json!(wings);
    }
    op
}

// --- Wiggum judges (goal-mode verifiers; full CRUD) -----------------------

pub(super) fn list_judges() -> Value {
    json!({ "op": "list_judges" })
}

pub(super) fn get_judge(name: &str) -> Value {
    json!({ "op": "get_judge", "name": name })
}

/// The full judge draft from the builder. An empty `prompt` is replaced by the
/// default goal-judge prompt sidecar-side; `new_name` (when non-empty) renames.
pub(super) fn save_judge(d: &JudgeDraft) -> Value {
    json!({
        "op": "save_judge",
        "name": d.name,
        "new_name": d.new_name,
        "model": d.model,
        "prompt": d.prompt,
        "enabled": d.enabled,
        "is_new": d.is_new,
    })
}

pub(super) fn delete_judge(name: &str) -> Value {
    json!({ "op": "delete_judge", "name": name })
}

pub(super) fn toggle_judge(name: &str) -> Value {
    json!({ "op": "toggle_judge", "name": name })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_arg_ops() {
        assert_eq!(cancel(), json!({"op": "cancel"}));
        assert_eq!(list_commands(), json!({"op": "list_commands"}));
        assert_eq!(list_agents(), json!({"op": "list_agents"}));
        assert_eq!(list_models(), json!({"op": "list_models"}));
        assert_eq!(status(), json!({"op": "status"}));
        assert_eq!(list_sessions(), json!({"op": "list_sessions"}));
        assert_eq!(list_mcp_servers(), json!({"op": "list_mcp_servers"}));
        assert_eq!(list_skills(), json!({"op": "list_skills"}));
        assert_eq!(list_agent_configs(), json!({"op": "list_agent_configs"}));
        assert_eq!(pause(), json!({"op": "pause"}));
        assert_eq!(resume(), json!({"op": "resume"}));
        assert_eq!(shutdown(), json!({"op": "shutdown"}));
    }

    #[test]
    fn turn_ops_carry_id_and_text() {
        assert_eq!(
            prompt(3, "hi", &[]),
            json!({"op": "prompt", "id": 3, "text": "hi"})
        );
        let imgs = vec!["BASE64PNG".to_string()];
        assert_eq!(
            prompt(5, "look", &imgs),
            json!({"op": "prompt", "id": 5, "text": "look", "images": ["BASE64PNG"]})
        );
        assert_eq!(
            command(4, "/clear"),
            json!({"op": "command", "id": 4, "text": "/clear"})
        );
        assert_eq!(
            complete(7, "/re", 3),
            json!({"op": "complete", "id": 7, "text": "/re", "cursor": 3})
        );
        assert_eq!(
            steer("focus tests", "now"),
            json!({"op": "steer", "text": "focus tests", "mode": "now"})
        );
    }

    #[test]
    fn config_ops() {
        assert_eq!(set_model("gpt"), json!({"op": "set_model", "name": "gpt"}));
        assert_eq!(
            set_agent("code-puppy"),
            json!({"op": "set_agent", "name": "code-puppy"})
        );
    }

    #[test]
    fn session_ops() {
        assert_eq!(
            load_session("auto_session_1", "autosave"),
            json!({"op": "load_session", "name": "auto_session_1", "source": "autosave"})
        );
        assert_eq!(
            preview_session("ctx", "context"),
            json!({"op": "preview_session", "name": "ctx", "source": "context"})
        );
    }

    #[test]
    fn ask_ops_use_one_op_with_cancelled_flag() {
        let answers = [AskAnswer {
            question_header: "Pick".into(),
            selected_options: vec!["A".into()],
            other_text: None,
        }];
        assert_eq!(
            ask_response("q1", &answers),
            json!({
                "op": "ask_response",
                "id": "q1",
                "cancelled": false,
                "answers": [{
                    "question_header": "Pick",
                    "selected_options": ["A"],
                    "other_text": null
                }]
            })
        );
        assert_eq!(
            ask_cancel("q1"),
            json!({"op": "ask_response", "id": "q1", "cancelled": true})
        );
    }

    #[test]
    fn mcp_ops() {
        assert_eq!(
            set_mcp_enabled("filesystem", true),
            json!({"op": "set_mcp_enabled", "name": "filesystem", "enabled": true})
        );
        assert_eq!(
            set_mcp_enabled("filesystem", false),
            json!({"op": "set_mcp_enabled", "name": "filesystem", "enabled": false})
        );
        let config = json!({"command": "npx", "args": ["-y", "server"], "env": {"K": "V"}});
        assert_eq!(
            add_mcp_server("fs", "stdio", &config),
            json!({
                "op": "add_mcp_server",
                "name": "fs",
                "type": "stdio",
                "config": {"command": "npx", "args": ["-y", "server"], "env": {"K": "V"}}
            })
        );
        let config =
            json!({"url": "https://example.com/sse", "headers": {"Authorization": "Bearer x"}});
        assert_eq!(
            add_mcp_server("remote", "sse", &config),
            json!({
                "op": "add_mcp_server",
                "name": "remote",
                "type": "sse",
                "config": {"url": "https://example.com/sse", "headers": {"Authorization": "Bearer x"}}
            })
        );
    }

    #[test]
    fn skill_ops() {
        assert_eq!(
            get_skill("git-flow"),
            json!({"op": "get_skill", "name": "git-flow"})
        );
        assert_eq!(
            set_skill_enabled("git-flow", true),
            json!({"op": "set_skill_enabled", "name": "git-flow", "enabled": true})
        );
        assert_eq!(
            set_skill_enabled("git-flow", false),
            json!({"op": "set_skill_enabled", "name": "git-flow", "enabled": false})
        );
        assert_eq!(
            save_skill("git-flow", "Git release flow", "## Steps\n", "user"),
            json!({
                "op": "save_skill",
                "name": "git-flow",
                "description": "Git release flow",
                "content": "## Steps\n",
                "scope": "user"
            })
        );
    }

    #[test]
    fn agent_config_ops() {
        assert_eq!(
            get_agent_config("helios"),
            json!({"op": "get_agent_config", "name": "helios"})
        );
        assert_eq!(
            delete_agent_config("my-bot"),
            json!({"op": "delete_agent_config", "name": "my-bot"})
        );
        assert_eq!(
            clone_agent_config("code-puppy"),
            json!({"op": "clone_agent_config", "name": "code-puppy"})
        );
        let draft = AgentConfigDraft {
            name: "my-bot".into(),
            display_name: "My Bot".into(),
            description: "does things".into(),
            system_prompt: "You are helpful.".into(),
            user_prompt: String::new(),
            model: "gpt".into(),
            tools: vec!["list_files".into(), "edit_file".into()],
            mcp_servers: vec!["serena".into()],
            scope: "user".into(),
        };
        assert_eq!(
            save_agent_config(&draft),
            json!({
                "op": "save_agent_config",
                "name": "my-bot",
                "display_name": "My Bot",
                "description": "does things",
                "system_prompt": "You are helpful.",
                "user_prompt": "",
                "model": "gpt",
                "tools": ["list_files", "edit_file"],
                "mcp_servers": ["serena"],
                "scope": "user"
            })
        );
    }

    #[test]
    fn kennel_ops() {
        assert_eq!(kennel_stats(), json!({"op": "kennel_stats"}));
        assert_eq!(kennel_list_wings(), json!({"op": "kennel_list_wings"}));
        // No wings -> the `wings` key is omitted (all wings).
        assert_eq!(
            kennel_recent(&[], 50),
            json!({"op": "kennel_recent", "limit": 50})
        );
        let wings = vec!["repo:/x".to_string(), "user:default".to_string()];
        assert_eq!(
            kennel_recent(&wings, 10),
            json!({"op": "kennel_recent", "limit": 10, "wings": ["repo:/x", "user:default"]})
        );
        assert_eq!(
            kennel_search("branch reorg", &[], 25),
            json!({"op": "kennel_search", "query": "branch reorg", "limit": 25})
        );
        assert_eq!(
            kennel_search("merge", &wings, 5),
            json!({"op": "kennel_search", "query": "merge", "limit": 5, "wings": ["repo:/x", "user:default"]})
        );
    }

    #[test]
    fn judge_ops() {
        assert_eq!(list_judges(), json!({"op": "list_judges"}));
        assert_eq!(
            get_judge("tests-pass"),
            json!({"op": "get_judge", "name": "tests-pass"})
        );
        assert_eq!(
            delete_judge("tests-pass"),
            json!({"op": "delete_judge", "name": "tests-pass"})
        );
        assert_eq!(
            toggle_judge("tests-pass"),
            json!({"op": "toggle_judge", "name": "tests-pass"})
        );
        let draft = JudgeDraft {
            name: "tests-pass".into(),
            new_name: String::new(),
            model: "gpt-5".into(),
            prompt: "Check the tests pass.".into(),
            enabled: true,
            is_new: true,
        };
        assert_eq!(
            save_judge(&draft),
            json!({
                "op": "save_judge",
                "name": "tests-pass",
                "new_name": "",
                "model": "gpt-5",
                "prompt": "Check the tests pass.",
                "enabled": true,
                "is_new": true
            })
        );
        let rename = JudgeDraft {
            name: "old".into(),
            new_name: "new".into(),
            model: "gpt-5".into(),
            prompt: String::new(),
            enabled: false,
            is_new: false,
        };
        assert_eq!(
            save_judge(&rename),
            json!({
                "op": "save_judge",
                "name": "old",
                "new_name": "new",
                "model": "gpt-5",
                "prompt": "",
                "enabled": false,
                "is_new": false
            })
        );
    }

    #[test]
    fn pending_response_ops() {
        assert_eq!(
            respond_input("p1", "yes"),
            json!({"op": "respond_input", "prompt_id": "p1", "value": "yes"})
        );
        assert_eq!(
            respond_confirmation("p2", true, None),
            json!({"op": "respond_confirmation", "prompt_id": "p2", "confirmed": true, "feedback": null})
        );
        assert_eq!(
            respond_confirmation("p2", false, Some("nope")),
            json!({"op": "respond_confirmation", "prompt_id": "p2", "confirmed": false, "feedback": "nope"})
        );
        assert_eq!(
            respond_selection("p3", 2, "blue"),
            json!({"op": "respond_selection", "prompt_id": "p3", "selected_index": 2, "selected_value": "blue"})
        );
    }
}
