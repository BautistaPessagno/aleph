// Import required modules from the LLM library for Google Gemini integration
use llm::{
    builder::{LLMBackend, LLMBuilder}, // Builder pattern components
    chat::ChatMessage,                 // Chat-related structures
};

#[tokio::main]
#[tauri::command]
pub async fn llms(query: &str) -> Result<String, String> {
    // Get Google API key from environment variable or use test key as fallback
    let api_key = std::env::var("OLLAMA_API_KEY").unwrap_or("ollama-key".into());

    // Initialize and configure the LLM client
    let llm = LLMBuilder::new()
        .backend(LLMBackend::Ollama) // Use Google as the LLM provider
        .api_key(api_key) // Set the API key
        .model("gpt-oss:20b") // Use Gemini Pro model
        .max_tokens(8512) // Limit response length
        .temperature(0.7) // Control response randomness (0.0-1.0)
        .stream(false) // Disable streaming responses
        // Optional: Set system prompt
        .system("You are a helpful AI assistant specialized in programming.")
        .build()
        .expect("Failed to build LLM (Ollama)");

    // Prepare conversation history with example messages

    let messages = vec![ChatMessage::user()
        .content("Dar una respuesta breve y concisa\n".to_owned() + query)
        .build()];

    // Send chat request and handle the response
    let ans = llm.chat(&messages).await.map_err(|e| e.to_string())?;

    println!("respuesta: {ans}");

    Ok(ans.to_string())
}
