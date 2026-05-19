use crate::core::session::{ActivationMode, ReplyPolicy, SessionKind};

pub trait Surface {
    fn name(&self) -> &'static str;
    fn session_kind(&self) -> SessionKind;
    fn activation_mode(&self) -> ActivationMode;
    fn reply_policy(&self, queued: bool) -> ReplyPolicy;
    fn supports_presence(&self) -> bool;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurfaceKind {
    Cli,
    TelegramDm,
    Voice,
}

impl Surface for SurfaceKind {
    fn name(&self) -> &'static str {
        match self {
            Self::Cli => "cli",
            Self::TelegramDm => "telegram",
            Self::Voice => "voice",
        }
    }

    fn session_kind(&self) -> SessionKind {
        match self {
            Self::Voice => SessionKind::Device,
            Self::Cli | Self::TelegramDm => SessionKind::Direct,
        }
    }

    fn activation_mode(&self) -> ActivationMode {
        match self {
            Self::Voice => ActivationMode::PushToTalk,
            Self::Cli | Self::TelegramDm => ActivationMode::Direct,
        }
    }

    fn reply_policy(&self, queued: bool) -> ReplyPolicy {
        match self {
            Self::Cli => ReplyPolicy::Immediate,
            Self::TelegramDm => {
                if queued {
                    ReplyPolicy::Debounced
                } else {
                    ReplyPolicy::Immediate
                }
            }
            Self::Voice => ReplyPolicy::FinalOnly,
        }
    }

    fn supports_presence(&self) -> bool {
        matches!(self, Self::TelegramDm)
    }
}
