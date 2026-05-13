use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::core::all::{ControllerFuture, RegisteredController};
use crate::core::{ControllerSchema, FieldSchema, TypeSchema};
use crate::rpc::RpcOutcome;

use super::types::{GameplayPresetInput, GameplayReviewAnalysisInput, GameplayReviewClipInput, GameplayReviewQuestionInput, GameplayReviewSessionInput};

#[derive(Deserialize)]
struct SessionIdParams {
    session_id: String,
}

pub fn all_controller_schemas() -> Vec<ControllerSchema> {
    vec![
        schemas("register_session"),
        schemas("analyze_session"),
        schemas("get_session"),
        schemas("list_sessions"),
        schemas("set_preset"),
        schemas("list_presets"),
        schemas("ask_session"),
        schemas("draft_clip_metadata"),
    ]
}

pub fn all_registered_controllers() -> Vec<RegisteredController> {
    vec![
        RegisteredController { schema: schemas("register_session"), handler: handle_register_session },
        RegisteredController { schema: schemas("analyze_session"), handler: handle_analyze_session },
        RegisteredController { schema: schemas("get_session"), handler: handle_get_session },
        RegisteredController { schema: schemas("list_sessions"), handler: handle_list_sessions },
        RegisteredController { schema: schemas("set_preset"), handler: handle_set_preset },
        RegisteredController { schema: schemas("list_presets"), handler: handle_list_presets },
        RegisteredController { schema: schemas("ask_session"), handler: handle_ask_session },
        RegisteredController { schema: schemas("draft_clip_metadata"), handler: handle_draft_clip_metadata },
    ]
}

pub fn schemas(function: &str) -> ControllerSchema {
    match function {
        "register_session" => ControllerSchema {
            namespace: "gameplay_review",
            function: "register_session",
            description: "Register a gameplay session from imported keyframes.",
            inputs: vec![FieldSchema { name: "session", ty: TypeSchema::Ref("GameplayReviewSessionInput"), comment: "Imported gameplay session metadata and frames.", required: true }],
            outputs: vec![json_output("session", "Stored gameplay review session.")],
        },
        "analyze_session" => ControllerSchema {
            namespace: "gameplay_review",
            function: "analyze_session",
            description: "Analyze a gameplay session, generate highlights, and draft clip metadata.",
            inputs: vec![FieldSchema { name: "analysis", ty: TypeSchema::Ref("GameplayReviewAnalysisInput"), comment: "Analysis request with optional highlight cap and platform targets.", required: true }],
            outputs: vec![json_output("session", "Analyzed gameplay review session.")],
        },
        "get_session" => ControllerSchema {
            namespace: "gameplay_review",
            function: "get_session",
            description: "Fetch one gameplay review session by id.",
            inputs: vec![FieldSchema { name: "session_id", ty: TypeSchema::String, comment: "Session identifier.", required: true }],
            outputs: vec![json_output("session", "Gameplay review session.")],
        },
        "list_sessions" => ControllerSchema {
            namespace: "gameplay_review",
            function: "list_sessions",
            description: "List gameplay review sessions stored in the workspace.",
            inputs: vec![FieldSchema { name: "game_id", ty: TypeSchema::Option(Box::new(TypeSchema::String)), comment: "Optional game filter.", required: false }],
            outputs: vec![json_output("sessions", "Gameplay review sessions.")],
        },
        "set_preset" => ControllerSchema {
            namespace: "gameplay_review",
            function: "set_preset",
            description: "Save a game-specific coaching preset.",
            inputs: vec![FieldSchema { name: "preset", ty: TypeSchema::Ref("GameplayPresetInput"), comment: "Game preset definition.", required: true }],
            outputs: vec![json_output("preset", "Saved gameplay review preset.")],
        },
        "list_presets" => ControllerSchema {
            namespace: "gameplay_review",
            function: "list_presets",
            description: "List stored gameplay coaching presets.",
            inputs: vec![],
            outputs: vec![json_output("presets", "Gameplay review presets.")],
        },
        "ask_session" => ControllerSchema {
            namespace: "gameplay_review",
            function: "ask_session",
            description: "Ask a question against a stored gameplay session.",
            inputs: vec![FieldSchema { name: "question", ty: TypeSchema::Ref("GameplayReviewQuestionInput"), comment: "Question and session id.", required: true }],
            outputs: vec![json_output("answer", "Question answer with matched highlights.")],
        },
        "draft_clip_metadata" => ControllerSchema {
            namespace: "gameplay_review",
            function: "draft_clip_metadata",
            description: "Draft clip titles, descriptions, and tags for one gameplay highlight.",
            inputs: vec![FieldSchema { name: "clip", ty: TypeSchema::Ref("GameplayReviewClipInput"), comment: "Clip draft request.", required: true }],
            outputs: vec![json_output("drafts", "Draft metadata for clip publishing.")],
        },
        _ => ControllerSchema {
            namespace: "gameplay_review",
            function: "unknown",
            description: "Unknown gameplay_review controller function.",
            inputs: vec![],
            outputs: vec![json_output("error", "Lookup error details.")],
        },
    }
}

fn json_output(name: &'static str, comment: &'static str) -> FieldSchema {
    FieldSchema { name, ty: TypeSchema::Json, comment, required: true }
}

fn deserialize_params<T: DeserializeOwned>(params: Map<String, Value>) -> Result<T, String> {
    serde_json::from_value(Value::Object(params)).map_err(|err| err.to_string())
}

fn handle_register_session(params: Map<String, Value>) -> ControllerFuture {
    Box::pin(async move {
        let payload = deserialize_params::<GameplayReviewSessionInput>(params)?;
        to_json(crate::openhuman::gameplay_review::rpc::register_session(payload).await?)
    })
}

fn handle_analyze_session(params: Map<String, Value>) -> ControllerFuture {
    Box::pin(async move {
        let payload = deserialize_params::<GameplayReviewAnalysisInput>(params)?;
        to_json(crate::openhuman::gameplay_review::rpc::analyze_session(payload).await?)
    })
}

fn handle_get_session(params: Map<String, Value>) -> ControllerFuture {
    Box::pin(async move {
        let payload = deserialize_params::<SessionIdParams>(params)?;
        to_json(crate::openhuman::gameplay_review::rpc::get_session(payload.session_id).await?)
    })
}

fn handle_list_sessions(params: Map<String, Value>) -> ControllerFuture {
    Box::pin(async move {
        #[derive(Deserialize)]
        struct Params {
            #[serde(default)]
            game_id: Option<String>,
        }
        let payload = deserialize_params::<Params>(params)?;
        to_json(crate::openhuman::gameplay_review::rpc::list_sessions(payload.game_id).await?)
    })
}

fn handle_set_preset(params: Map<String, Value>) -> ControllerFuture {
    Box::pin(async move {
        let payload = deserialize_params::<GameplayPresetInput>(params)?;
        to_json(crate::openhuman::gameplay_review::rpc::set_preset(payload).await?)
    })
}

fn handle_list_presets(_params: Map<String, Value>) -> ControllerFuture {
    Box::pin(async move { to_json(crate::openhuman::gameplay_review::rpc::list_presets().await?) })
}

fn handle_ask_session(params: Map<String, Value>) -> ControllerFuture {
    Box::pin(async move {
        let payload = deserialize_params::<GameplayReviewQuestionInput>(params)?;
        to_json(crate::openhuman::gameplay_review::rpc::ask_session(payload).await?)
    })
}

fn handle_draft_clip_metadata(params: Map<String, Value>) -> ControllerFuture {
    Box::pin(async move {
        let payload = deserialize_params::<GameplayReviewClipInput>(params)?;
        to_json(crate::openhuman::gameplay_review::rpc::draft_clip_metadata(payload).await?)
    })
}

fn to_json<T: serde::Serialize>(value: T) -> ControllerFuture {
    Box::pin(async move { serde_json::to_value(value).map_err(|err| err.to_string()) })
}
