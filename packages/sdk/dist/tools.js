import { clientGet, clientPost } from "./client.js";
const CANONICAL_TOOLS = [
    {
        name: "memory_query",
        description: "Query the memory pack with semantic search. Use when you need to find relevant context from indexed content.",
        parameters: {
            type: "object",
            properties: {
                query: { type: "string", description: "Search query" },
                pack_uri: {
                    type: "string",
                    description: "Optional cloud pack URI (memkit://users/... or memkit://orgs/...)",
                },
                top_k: { type: "number", description: "Max results (default 8)" },
                use_reranker: { type: "boolean", description: "Use reranker (default true)" },
            },
            required: ["query"],
        },
    },
    {
        name: "memory_status",
        description: "Get memory pack status: indexed state, sources, pack path.",
        parameters: {
            type: "object",
            properties: {},
            required: [],
        },
    },
    {
        name: "memory_sources",
        description: "List configured memory source roots.",
        parameters: {
            type: "object",
            properties: {},
            required: [],
        },
    },
    {
        name: "memory_add",
        description: "Add documents or conversation to the memory pack.",
        parameters: {
            type: "object",
            properties: {
                documents: {
                    type: "array",
                    items: { type: "string" },
                    description: "URLs, file paths, or inline content",
                },
                conversation: {
                    type: "array",
                    items: {
                        type: "object",
                        properties: {
                            role: { type: "string" },
                            content: { type: "string" },
                        },
                        required: ["role", "content"],
                    },
                    description: "Conversation transcript",
                },
            },
            required: [],
        },
    },
];
export function getToolsForProvider(provider) {
    if (provider === "anthropic") {
        return CANONICAL_TOOLS.map((t) => ({
            name: t.name,
            description: t.description,
            input_schema: t.parameters,
        }));
    }
    return CANONICAL_TOOLS.map((t) => ({
        type: "function",
        function: {
            name: t.name,
            description: t.description,
            parameters: t.parameters,
        },
    }));
}
export async function executeToolInternal(name, args) {
    switch (name) {
        case "memory_query": {
            const query = String(args.query ?? "");
            const pack_uri = typeof args.pack_uri === "string" ? String(args.pack_uri) : undefined;
            const top_k = Number(args.top_k ?? 8);
            const use_reranker = args.use_reranker !== false;
            const body = {
                query,
                top_k,
                use_reranker,
                raw: false,
            };
            if (pack_uri)
                body.pack_uri = pack_uri;
            const result = await clientPost("/query", body);
            return result;
        }
        case "memory_status": {
            const result = await clientGet("/status");
            return result;
        }
        case "memory_sources": {
            const result = (await clientGet("/status"));
            return { sources: result.sources ?? [] };
        }
        case "memory_add": {
            const body = {};
            if (args.documents) {
                body.documents = args.documents.map((s) => s.startsWith("http://") || s.startsWith("https://")
                    ? { type: "url", value: s }
                    : { type: "content", value: s });
            }
            if (args.conversation) {
                body.conversation = args.conversation;
            }
            await clientPost("/add", body);
            return { status: "ok" };
        }
        default:
            throw new Error(`memkit: unknown tool ${name}`);
    }
}
