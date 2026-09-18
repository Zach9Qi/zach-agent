//! 文件与多模态数据载荷模块

use std::collections::HashMap;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use bytes::Bytes;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// 文件与多模态数据载荷
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FileData {
    /// 原始字节数据，内部基于 `Bytes` 实现高效的跨线程共享与零拷贝切片
    Data {
        #[serde(
            serialize_with = "serialize_bytes_base64",
            deserialize_with = "deserialize_bytes_base64"
        )]
        data: Bytes,
    },
    /// 指向文件或资源的 URL
    Url { url: String },
    /// 厂商侧已有文件的引用 ID（例如 OpenAI file-abc123）
    Reference { reference: HashMap<String, String> },
    /// 内联纯文本文档
    Text { text: String },
}

impl FileData {
    /// 从原始字节构造
    pub fn from_bytes(data: impl Into<Bytes>) -> Self {
        Self::Data { data: data.into() }
    }

    /// 从普通字符串构造内联文本文件
    pub fn from_text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }

    /// 从 URL 构造文件载荷
    pub fn from_url(url: impl Into<String>) -> Self {
        Self::Url { url: url.into() }
    }

    /// 构造厂商侧引用
    pub fn from_reference(provider: impl Into<String>, file_id: impl Into<String>) -> Self {
        let mut map = HashMap::new();
        map.insert(provider.into(), file_id.into());
        Self::Reference { reference: map }
    }

    /// 尝试以二进制字节切片形式读取数据（如果是 Data 或 Text）
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Self::Data { data } => Some(data.as_ref()),
            Self::Text { text } => Some(text.as_bytes()),
            _ => None,
        }
    }
}

/// 序列化 Bytes 为 base64 字符串
fn serialize_bytes_base64<S>(bytes: &Bytes, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let encoded = BASE64.encode(bytes.as_ref());
    serializer.serialize_str(&encoded)
}

/// 从 base64 字符串或字节数组反序列化为 Bytes
fn deserialize_bytes_base64<'de, D>(deserializer: D) -> Result<Bytes, D::Error>
where
    D: Deserializer<'de>,
{
    struct BytesVisitor;

    impl<'de> serde::de::Visitor<'de> for BytesVisitor {
        type Value = Bytes;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("base64 编码的字符串或字节数组")
        }

        fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            BASE64
                .decode(v.as_bytes())
                .map(Bytes::from)
                .map_err(serde::de::Error::custom)
        }

        fn visit_bytes<E>(self, v: &[u8]) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(Bytes::copy_from_slice(v))
        }

        fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
        where
            A: serde::de::SeqAccess<'de>,
        {
            let mut vec = Vec::new();
            while let Some(byte) = seq.next_element::<u8>()? {
                vec.push(byte);
            }
            Ok(Bytes::from(vec))
        }
    }

    deserializer.deserialize_any(BytesVisitor)
}
