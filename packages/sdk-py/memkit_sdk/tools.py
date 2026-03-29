from .client import client_get, client_post

CANONICAL_TOOLS = [
    {
        "name": "memory_query",
        "description": "Query the memory pack with semantic search. Use when you need to find relevant context from indexed content.",
        "parameters": {
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "Search query"},
                "pack_uri": {
                    "type": "string",
                    "description": "Optional cloud pack URI (memkit://users/... or memkit://orgs/...)",
                },
                "top_k": {"type": "number", "description": "Max results (default 8)"},
                "use_reranker": {"type": "boolean", "description": "Use reranker (default true)"},
            },
            "required": ["query"],
        },
    },
    {
        "name": "memory_status",
        "description": "Get memory pack status: indexed state, sources, pack path.",
        "parameters": {"type": "object", "properties": {}, "required": []},
    },
    {
        "name": "memory_sources",
        "description": "List configured memory source roots.",
        "parameters": {"type": "object", "properties": {}, "required": []},
    },
    {
        "name": "memory_add",
        "description": "Add documents or conversation to the memory pack.",
        "parameters": {
            "type": "object",
            "properties": {
                "documents": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "URLs, file paths, or inline content",
                },
                "conversation": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "role": {"type": "string"},
                            "content": {"type": "string"},
                        },
                        "required": ["role", "content"],
                    },
                    "description": "Conversation transcript",
                },
            },
            "required": [],
        },
    },
]


def get_tools_for_provider(provider: str) -> list:
    if provider == "anthropic":
        return [
            {
                "name": t["name"],
                "description": t["description"],
                "input_schema": t["parameters"],
            }
            for t in CANONICAL_TOOLS
        ]
    return [
        {
            "type": "function",
            "function": {
                "name": t["name"],
                "description": t["description"],
                "parameters": t["parameters"],
            },
        }
        for t in CANONICAL_TOOLS
    ]


def execute_tool_internal(name: str, args: dict) -> dict | str:
    match name:
        case "memory_query":
            query = str(args.get("query", ""))
            pack_uri = args.get("pack_uri")
            top_k = int(args.get("top_k", 8))
            use_reranker = args.get("use_reranker", True) is not False
            body = {"query": query, "top_k": top_k, "use_reranker": use_reranker, "raw": False}
            if pack_uri:
                body["pack_uri"] = str(pack_uri)
            return client_post("/query", body)
        case "memory_status":
            return client_get("/status")
        case "memory_sources":
            result = client_get("/status")
            return {"sources": result.get("sources", [])}
        case "memory_add":
            body: dict = {}
            if args.get("documents"):
                body["documents"] = [
                    {"type": "url", "value": s}
                    if s.startswith("http://") or s.startswith("https://")
                    else {"type": "content", "value": s}
                    for s in args["documents"]
                ]
            if args.get("conversation"):
                body["conversation"] = args["conversation"]
            client_post("/add", body)
            return {"status": "ok"}
        case _:
            raise ValueError(f"memkit: unknown tool {name}")
