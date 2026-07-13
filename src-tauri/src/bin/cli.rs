//! NeeCoder CLI - Command-line interface for AI-assisted coding
//!
//! Usage:
//!   neecoder-cli                    # Start interactive REPL
//!   neecoder-cli --help             # Show help
//!   neecoder-cli --model gpt-4o     # Specify model
//!   neecoder-cli --provider openai  # Specify provider

use clap::Parser;
use neecoder_tauri_lib::config::LlmProvider;
use neecoder_tauri_lib::llm::{ChatMessage, ChatRequestParams, stream_chat};
use std::io::{self, Write};

#[derive(Parser)]
#[command(name = "neecoder-cli")]
#[command(about = "NeeCoder AI Coding Assistant - CLI Mode")]
#[command(version)]
struct Cli {
    /// LLM provider (openai, deepseek, anthropic, ollama)
    #[arg(short, long, default_value = "openai")]
    provider: String,

    /// Model name
    #[arg(short, long, default_value = "gpt-4o")]
    model: String,

    /// API key (or set via environment variable NEECODER_API_KEY)
    #[arg(short, long)]
    api_key: Option<String>,

    /// Base URL for API (optional)
    #[arg(long)]
    base_url: Option<String>,

    /// System prompt
    #[arg(short, long)]
    system: Option<String>,

    /// Enable streaming output
    #[arg(short, long, default_value = "true")]
    stream: bool,
}

fn parse_provider(s: &str) -> LlmProvider {
    match s.to_lowercase().as_str() {
        "openai" => LlmProvider::OpenAI,
        "deepseek" => LlmProvider::DeepSeek,
        "anthropic" | "claude" => LlmProvider::Anthropic,
        "ollama" => LlmProvider::Ollama,
        _ => LlmProvider::OpenAI,
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    // Validate API key
    let api_key = cli.api_key.or_else(|| std::env::var("NEECODER_API_KEY").ok()).unwrap_or_else(|| {
        eprintln!("Error: API key is required. Set via --api-key or NEECODER_API_KEY env var.");
        std::process::exit(1);
    });

    let provider = parse_provider(&cli.provider);
    let system_prompt = cli.system.unwrap_or_else(|| {
        "You are NeeCoder, an AI coding assistant. Help users with programming tasks.".to_string()
    });

    println!("NeeCoder CLI v0.1.0");
    println!("Provider: {}, Model: {}", cli.provider, cli.model);
    println!("Type 'exit' or 'quit' to leave, 'help' for commands.\n");

    // Conversation history
    let mut messages: Vec<ChatMessage> = Vec::new();

    // REPL loop
    loop {
        print!("> ");
        io::stdout().flush()?;

        let mut input = String::new();
        if io::stdin().read_line(&mut input)? == 0 {
            break; // EOF
        }

        let input = input.trim();
        if input.is_empty() {
            continue;
        }

        // Handle commands
        match input.to_lowercase().as_str() {
            "exit" | "quit" | "q" => {
                println!("Goodbye!");
                break;
            }
            "help" | "h" | "?" => {
                println!("Commands:");
                println!("  exit, quit, q  - Exit the CLI");
                println!("  help, h, ?     - Show this help");
                println!("  clear          - Clear conversation history");
                println!("  history        - Show conversation history");
                println!();
                continue;
            }
            "clear" => {
                messages.clear();
                println!("Conversation history cleared.\n");
                continue;
            }
            "history" => {
                println!("Conversation history ({} messages):", messages.len());
                for (i, msg) in messages.iter().enumerate() {
                    let preview = if msg.content.len() > 100 {
                        format!("{}...", &msg.content[..100])
                    } else {
                        msg.content.clone()
                    };
                    println!("  [{}] {}: {}", i, msg.role, preview);
                }
                println!();
                continue;
            }
            _ => {}
        }

        // Add user message
        messages.push(ChatMessage {
            role: "user".to_string(),
            content: input.to_string(),
            images: None,
            tool_calls: None,
            tool_call_id: None,
        });

        // Build request
        let request = ChatRequestParams {
            model: cli.model.clone(),
            messages: messages.clone(),
            system: system_prompt.clone(),
            max_tokens: 4096,
            temperature: 0.7,
            thinking_enabled: false,
            thinking_budget: 0,
        };

        // Call LLM
        if cli.stream {
            // Streaming mode
            print!("Assistant: ");
            io::stdout().flush()?;

            let mut response_content = String::new();
            let result = stream_chat(
                &provider,
                &api_key,
                cli.base_url.as_deref(),
                request,
                |chunk| {
                    print!("{}", chunk);
                    io::stdout().flush().ok();
                    response_content.push_str(&chunk);
                    Ok(())
                },
                None,
            )
            .await;

            println!(); // New line after streaming

            match result {
                Ok(_) => {
                    messages.push(ChatMessage {
                        role: "assistant".to_string(),
                        content: response_content,
                        images: None,
                        tool_calls: None,
                        tool_call_id: None,
                    });
                }
                Err(e) => {
                    eprintln!("\nError: {}\n", e);
                    // Remove the failed user message
                    messages.pop();
                }
            }
        } else {
            // Non-streaming mode - use stream_chat but collect all tokens
            let mut response_content = String::new();
            let result = stream_chat(
                &provider,
                &api_key,
                cli.base_url.as_deref(),
                request,
                |chunk| {
                    response_content.push_str(&chunk);
                    Ok(())
                },
                None,
            )
            .await;

            match result {
                Ok(_) => {
                    println!("Assistant: {}\n", response_content);
                    messages.push(ChatMessage {
                        role: "assistant".to_string(),
                        content: response_content,
                        images: None,
                        tool_calls: None,
                        tool_call_id: None,
                    });
                }
                Err(e) => {
                    eprintln!("Error: {}\n", e);
                    // Remove the failed user message
                    messages.pop();
                }
            }
        }
    }

    Ok(())
}
