import { useState, useEffect } from "react";

interface Props {
  language: string;
  code: string;
  customStyle?: React.CSSProperties;
}

export default function SyntaxHighlighterWrapper({ language, code, customStyle }: Props) {
  const [Highlighter, setHighlighter] = useState<any>(null);
  const [style, setStyle] = useState<any>(null);

  useEffect(() => {
    Promise.all([
      import("react-syntax-highlighter"),
      import("react-syntax-highlighter/dist/esm/styles/prism"),
    ]).then(([hlMod, styleMod]) => {
      setHighlighter(() => hlMod.Prism);
      setStyle(styleMod.oneDark);
    });
  }, []);

  if (!Highlighter || !style) {
    return <pre className="code-fallback"><code>{code}</code></pre>;
  }

  return (
    <Highlighter
      style={style}
      language={language}
      PreTag="div"
      customStyle={{ margin: 0, borderRadius: "0 0 6px 6px", fontSize: 12, ...customStyle }}
    >
      {code}
    </Highlighter>
  );
}
