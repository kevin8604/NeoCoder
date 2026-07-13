use super::{Tool, ToolContext};

pub struct GenerateDiagram;

#[async_trait::async_trait]
impl Tool for GenerateDiagram {
    fn name(&self) -> &str {
        "generate_diagram"
    }

    async fn execute(&self, args: serde_json::Value, _ctx: &ToolContext) -> String {
        let mermaid_code = args["mermaid"].as_str().unwrap_or("");
        let diagram_type = args["type"].as_str().unwrap_or("mermaid");

        if mermaid_code.is_empty() {
            return "Error: mermaid code is required".to_string();
        }

        // Generate a unique filename for the diagram
        let timestamp = chrono::Utc::now().timestamp_millis();
        let filename = format!("diagram_{}.mmd", timestamp);

        // Write to temp directory so frontend can read and render it
        let temp_dir = std::env::temp_dir().join("neecoder_diagrams");
        let _ = std::fs::create_dir_all(&temp_dir);
        let file_path = temp_dir.join(&filename);

        match std::fs::write(&file_path, mermaid_code) {
            Ok(()) => {
                let preview = mermaid_code.lines().take(5).collect::<Vec<_>>().join("\n");
                format!(
                    "[DIAGRAM:{}]\nDiagram type: {}\nSaved to: {}\n\nPreview:\n{}\n\n```mermaid\n{}\n```",
                    file_path.display(),
                    diagram_type,
                    file_path.display(),
                    preview,
                    mermaid_code
                )
            }
            Err(e) => format!("Error generating diagram: {}", e),
        }
    }
}
