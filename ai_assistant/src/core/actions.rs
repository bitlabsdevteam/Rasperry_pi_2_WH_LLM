#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MemoryActionScope {
    Personal,
    Knowledge,
    Runtime,
}

impl MemoryActionScope {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Personal => "personal",
            Self::Knowledge => "knowledge",
            Self::Runtime => "runtime",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AssistantAction {
    ReplyText {
        text: String,
    },
    RunTool {
        command: String,
        args: Vec<String>,
        reason: String,
    },
    SearchMemory {
        scope: MemoryActionScope,
        query: String,
    },
    AskFollowup {
        question: String,
    },
    Defer {
        notice: String,
    },
    ScheduleTask {
        title: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActionTrace {
    pub kind: String,
    pub detail: String,
}

impl ActionTrace {
    pub fn new(kind: &str, detail: impl Into<String>) -> Self {
        Self {
            kind: kind.to_string(),
            detail: detail.into(),
        }
    }
}
