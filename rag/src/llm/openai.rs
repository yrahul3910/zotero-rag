use super::base::{ApiClient, ApiResponse, ChatHistoryItem, UserMessage};
use super::errors::LLMError;
use super::http_client::{HttpClient, ReqwestClient};
use crate::common;
use futures::stream;
use futures::StreamExt;
use http::HeaderMap;
use lancedb::arrow::arrow_schema::{DataType, Field};
use lancedb::embeddings::EmbeddingFunction;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::env;
use std::sync::Arc;

/// A client for OpenAI's chat completions API
#[derive(Debug, Clone)]
pub struct OpenAIClient<T: HttpClient = ReqwestClient> {
    pub client: T,
}

impl<T: HttpClient + Default> Default for OpenAIClient<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> OpenAIClient<T>
where
    T: HttpClient + Default,
{
    /// Creates a new OpenAIClient instance
    pub fn new() -> Self {
        Self {
            client: T::default(),
        }
    }

    // Async version to handle the embedding computation
    async fn compute_embeddings_async(
        &self,
        source: Arc<dyn arrow_array::Array>,
    ) -> Result<Arc<dyn arrow_array::Array>, LLMError> {
        let source_array = arrow_array::cast::as_string_array(&source);
        let texts: Vec<String> = source_array.iter().map(|s| s.unwrap().to_owned()).collect();

        // Create a stream of futures
        let futures = texts
            .iter()
            .map(|text| common::get_openai_embedding(text.clone()));

        // Convert to a stream and process with buffer_unordered to limit concurrency
        let max_concurrent = std::env::var("MAX_CONCURRENT_REQUESTS")
            .unwrap_or_else(|_| "5".to_string())
            .parse::<usize>()
            .unwrap_or(5);

        // Process futures with limited concurrency
        let results = stream::iter(futures)
            .buffer_unordered(max_concurrent)
            .collect::<Vec<_>>()
            .await;

        // Process results and construct Arrow array
        let mut embeddings: Vec<Vec<f32>> = Vec::with_capacity(texts.len());
        for result in results {
            match result {
                Ok(embedding) => embeddings.push(embedding),
                Err(e) => return Err(e),
            }
        }

        // Convert to Arrow FixedSizeListArray
        let embedding_dim = if embeddings.is_empty() {
            1536 // default for text-embedding-3-small
        } else {
            embeddings[0].len()
        };

        let flattened: Vec<f32> = embeddings.iter().flatten().copied().collect();
        let values = arrow_array::Float32Array::from(flattened);

        let list_array = arrow_array::FixedSizeListArray::try_new(
            Arc::new(Field::new("item", DataType::Float32, false)),
            embedding_dim as i32,
            Arc::new(values),
            None,
        )
        .map_err(|e| {
            LLMError::GenericLLMError(format!("Failed to create FixedSizeListArray: {}", e))
        })?;

        Ok(Arc::new(list_array) as Arc<dyn arrow_array::Array>)
    }

    // This is our internal implementation that works with LLMError
    // Note that this is also copied in AnthropicClient.
    pub fn compute_embeddings_internal(
        &self,
        source: Arc<dyn arrow_array::Array>,
    ) -> Result<Arc<dyn arrow_array::Array>, LLMError> {
        // Convert to a synchronous operation because the trait expects a Result, not a Future
        let rt = tokio::runtime::Runtime::new()
            .map_err(|_| LLMError::GenericLLMError("Could not create tokio runtime".to_string()))?;

        rt.block_on(self.compute_embeddings_async(source))
    }
}

#[derive(Serialize, Deserialize)]
struct OpenAIRequest {
    model: String,
    messages: Vec<ChatHistoryItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
}

impl From<UserMessage> for OpenAIRequest {
    fn from(msg: UserMessage) -> OpenAIRequest {
        let model = env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-3.5-turbo".to_string());
        let max_tokens = env::var("OPENAI_MAX_TOKENS")
            .ok()
            .and_then(|s| s.parse().ok());
        let mut messages = msg.chat_history.clone();
        messages.push(ChatHistoryItem {
            role: "user".to_owned(),
            content: msg.message.clone(),
        });

        OpenAIRequest {
            model,
            messages,
            max_tokens,
        }
    }
}

#[derive(Serialize, Deserialize, Clone)]
struct OpenAIUsage {
    prompt_tokens: u32,
    completion_tokens: u32,
    total_tokens: u32,
}

#[derive(Serialize, Deserialize, Clone)]
struct OpenAIChoiceMessage {
    role: String,
    content: String,
}

#[derive(Serialize, Deserialize, Clone)]
struct OpenAIChoice {
    message: OpenAIChoiceMessage,
    finish_reason: Option<String>,
    index: u32,
}

#[derive(Serialize, Deserialize, Clone)]
struct OpenAIResponse {
    id: String,
    object: String,
    created: u64,
    model: String,
    usage: OpenAIUsage,
    choices: Vec<OpenAIChoice>,
}

impl<T: HttpClient> ApiClient for OpenAIClient<T> {
    async fn send_message(
        &self,
        message: &UserMessage,
    ) -> Result<ApiResponse, super::errors::LLMError> {
        let key = env::var("OPENAI_API_KEY")?;

        let mut headers = HeaderMap::new();
        headers.insert("Authorization", format!("Bearer {}", key).parse().unwrap());
        headers.insert("content-type", "application/json".parse().unwrap());

        let req_body: OpenAIRequest = message.clone().into();
        let res = self
            .client
            .post_json(
                "https://api.openai.com/v1/chat/completions",
                headers,
                &req_body,
            )
            .await?;

        let body = res.text().await?;

        let json: serde_json::Value = match serde_json::from_str(&body) {
            Ok(json) => json,
            Err(_) => return Err(super::errors::LLMError::DeserializationError(body)),
        };

        let response: OpenAIResponse = match serde_json::from_value(json) {
            Ok(response) => response,
            Err(_) => return Err(super::errors::LLMError::DeserializationError(body)),
        };

        let choice = &response.choices[0];
        Ok(ApiResponse {
            content: choice.message.content.clone(),
            input_tokens: response.usage.prompt_tokens,
            output_tokens: response.usage.completion_tokens,
        })
    }
}

/// Implements the LanceDB EmbeddingFunction trait for OpenAI client. This is the same code
/// as the one in AnthropicClient verbatim--I made a judgement call that two copies are okay;
/// when we hit a place where we need a third copy, we'll refactor.
///
/// Maintainers should note that any updates here should also be reflected in AnthropicClient.
impl EmbeddingFunction for OpenAIClient {
    fn name(&self) -> &str {
        "OpenAI"
    }

    fn source_type(&self) -> Result<Cow<DataType>, lancedb::Error> {
        Ok(Cow::Owned(DataType::Utf8))
    }

    fn dest_type(&self) -> Result<Cow<DataType>, lancedb::Error> {
        Ok(Cow::Owned(DataType::FixedSizeList(
            Arc::new(lancedb::arrow::arrow_schema::Field::new(
                "item",
                DataType::Float32,
                false,
            )),
            1536, // text-embedding-3-small size
        )))
    }

    fn compute_source_embeddings(
        &self,
        source: Arc<dyn arrow_array::Array>,
    ) -> Result<Arc<dyn arrow_array::Array>, lancedb::Error> {
        // Call our internal implementation and map LLMError to lancedb::Error
        match self.compute_embeddings_internal(source) {
            Ok(result) => Ok(result),
            Err(e) => Err(lancedb::Error::Other {
                message: e.to_string(),
                source: None,
            }),
        }
    }

    fn compute_query_embeddings(
        &self,
        input: Arc<dyn arrow_array::Array>,
    ) -> Result<Arc<dyn arrow_array::Array>, lancedb::Error> {
        // For queries, we don't need concurrency since it's typically a single query
        // Just reuse the same implementation with the expectation it's usually for one item
        match self.compute_embeddings_internal(input) {
            Ok(result) => Ok(result),
            Err(e) => Err(lancedb::Error::Other {
                message: e.to_string(),
                source: None,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{OpenAIClient, OpenAIResponse, OpenAIUsage, OpenAIChoice, OpenAIChoiceMessage};
    use crate::llm::base::{ApiClient, UserMessage};
    use crate::llm::http_client::{MockHttpClient, ReqwestClient};
    use dotenv::dotenv;
    use std::env;

    #[tokio::test]
    async fn test_request_works() {
        dotenv().ok();

        if env::var("CI").is_ok() {
            // Skip this test in CI environments
            return;
        }

        let client = OpenAIClient::<ReqwestClient>::default();
        let message = UserMessage {
            chat_history: Vec::new(),
            message: "Hello!".to_owned(),
        };
        let res = client.send_message(&message).await;
        assert!(res.is_ok());
        dbg!(res.unwrap());
    }

    #[tokio::test]
    async fn test_request_with_mock() {
        // Load environment variables from .env file
        dotenv().ok();
        
        // Create a proper OpenAIResponse that matches the structure we expect to deserialize
        let mock_response = OpenAIResponse {
            id: "mock-id".to_string(),
            object: "chat.completion".to_string(),
            created: 1234567890,
            model: "gpt-3.5-turbo".to_string(),
            usage: OpenAIUsage {
                prompt_tokens: 5,
                completion_tokens: 10,
                total_tokens: 15,
            },
            choices: vec![OpenAIChoice {
                message: OpenAIChoiceMessage {
                    role: "assistant".to_string(),
                    content: "Hi there!".to_string(),
                },
                finish_reason: Some("stop".to_string()),
                index: 0,
            }],
        };
        
        let mock_http_client = MockHttpClient::new(mock_response);
        let mock_client = OpenAIClient {
            client: mock_http_client,
        };

        let message = UserMessage {
            chat_history: Vec::new(),
            message: "Hello!".to_owned(),
        };
        let res = mock_client.send_message(&message).await;
        
        // Debug the error if there is one
        if res.is_err() {
            println!("OpenAI test error: {:?}", res.as_ref().err());
        }
        
        assert!(res.is_ok());
        let res = res.unwrap();
        assert_eq!(res.content, "Hi there!");
        assert_eq!(res.input_tokens, 5);
        assert_eq!(res.output_tokens, 10);
    }
}
