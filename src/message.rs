use anyhow::{self, Result};
use async_tungstenite::tungstenite::Message;
use serde::{Deserialize, Serialize};
use serde_json;
use std::convert::TryFrom;

use crate::view_state::ViewState;

// use crate::render::view_state::ViewState;

#[derive(Debug, Serialize, Deserialize, Clone, Copy)]
pub struct ViewportSize {
    pub width: u32,
    pub height: u32,
}
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ClientConfig {
    pub id: String,
    pub viewport: ViewportSize,
    pub video_scaling: f32,
    pub gpu: bool,
    pub lossless: bool,
    pub bitrate: f32,
    pub preset: String,
    pub fullrange: bool,
}
impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            id: "foo".to_owned(),
            viewport: ViewportSize {
                width: 64,
                height: 64,
            },
            video_scaling: 1.0,
            gpu: false,
            lossless: false,
            bitrate: 4_f32,
            preset: "default".to_owned(),
            fullrange: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub struct Protocols {
    pub layout: Vec<LayoutCfg>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub struct LayoutCfg {
    pub name: String,
    pub rows: usize,
    pub columns: usize,
    pub panes: Vec<PaneCfg>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub struct PaneCfg {
    pub case: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub struct CaseMeta {
    pub key: String,
    pub number_of_images: usize,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AppMessage {
    Connect(Vec<ClientConfig>),
    Ice {
        candidate: String,
        #[serde(rename = "sdpMLineIndex")]
        sdp_mline_index: u32,
    },
    Sdp {
        #[serde(rename = "type")]
        type_: String,
        sdp: String,
    },
    GetCases,
    Case((Option<Protocols>, Vec<CaseMeta>)),
    Close,
    Reconfigure(Vec<ClientConfig>),
}

impl TryFrom<Message> for AppMessage {
    type Error = anyhow::Error;
    fn try_from(value: Message) -> Result<Self> {
        match value.to_text() {
            Ok(value) => Ok(serde_json::from_str(value)?),
            Err(_) => Err(anyhow::anyhow!("Message is not text")),
        }
    }
}

impl TryFrom<AppMessage> for Message {
    type Error = anyhow::Error;
    fn try_from(value: AppMessage) -> Result<Self> {
        Ok(Message::from(serde_json::to_string(&value)?))
    }
}

#[derive(Debug, Copy, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub struct LayoutRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub struct PaneState {
    pub layout: LayoutRect,
    pub view_state: ViewState,
    pub key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub struct RenderState {
    pub panes: Vec<PaneState>,
    pub layout: LayoutRect,
    pub seq: u64,
    pub timestamp: f32,
    pub snapshot: bool,
    pub bitrate: f32,
}

impl RenderState {
    pub fn with_seq(seq: u64) -> Self {
        Self {
            seq,
            ..Self::default()
        }
    }
}
impl Default for RenderState {
    fn default() -> Self {
        Self {
            panes: vec![PaneState {
                view_state: ViewState::new(),
                key: None,
                layout: LayoutRect::default(),
            }],
            layout: LayoutRect::default(),
            seq: 0,
            timestamp: 0.0_f32,
            snapshot: false,
            bitrate: 4f32,
        }
    }
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DataMessage {
    NewState(RenderState),
    Eof(u64),
}

impl TryFrom<String> for DataMessage {
    type Error = anyhow::Error;
    fn try_from(value: String) -> Result<Self> {
        Ok(serde_json::from_str(&value)?)
    }
}

impl TryFrom<DataMessage> for String {
    type Error = anyhow::Error;
    fn try_from(value: DataMessage) -> Result<Self> {
        Ok(serde_json::to_string(&value)?)
    }
}
