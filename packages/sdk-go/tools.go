package sdk

import (
	"context"
	"fmt"
	"strings"
)

var canonicalTools = []map[string]any{
	{
		"name":        "memory_query",
		"description": "Query the memory pack with semantic search. Use when you need to find relevant context from indexed content.",
		"parameters": map[string]any{
			"type": "object",
			"properties": map[string]any{
				"query":        map[string]any{"type": "string", "description": "Search query"},
				"pack_uri":     map[string]any{"type": "string", "description": "Optional cloud pack URI (memkit://users/... or memkit://orgs/...)"},
				"top_k":        map[string]any{"type": "number", "description": "Max results (default 8)"},
				"use_reranker": map[string]any{"type": "boolean", "description": "Use reranker (default true)"},
			},
			"required": []any{"query"},
		},
	},
	{
		"name":        "memory_status",
		"description": "Get memory pack status: indexed state, sources, pack path.",
		"parameters": map[string]any{
			"type":       "object",
			"properties": map[string]any{},
			"required":   []any{},
		},
	},
	{
		"name":        "memory_sources",
		"description": "List configured memory source roots.",
		"parameters": map[string]any{
			"type":       "object",
			"properties": map[string]any{},
			"required":   []any{},
		},
	},
	{
		"name":        "memory_add",
		"description": "Add documents or conversation to the memory pack.",
		"parameters": map[string]any{
			"type": "object",
			"properties": map[string]any{
				"documents": map[string]any{
					"type":        "array",
					"items":       map[string]any{"type": "string"},
					"description": "URLs, file paths, or inline content",
				},
				"conversation": map[string]any{
					"type": "array",
					"items": map[string]any{
						"type": "object",
						"properties": map[string]any{
							"role":    map[string]any{"type": "string"},
							"content": map[string]any{"type": "string"},
						},
						"required": []any{"role", "content"},
					},
					"description": "Conversation transcript",
				},
			},
			"required": []any{},
		},
	},
}

func GetToolsForProvider(provider string) []any {
	out := make([]any, len(canonicalTools))
	for i, t := range canonicalTools {
		if provider == "anthropic" {
			out[i] = map[string]any{
				"name":         t["name"],
				"description":  t["description"],
				"input_schema": t["parameters"],
			}
		} else {
			out[i] = map[string]any{
				"type": "function",
				"function": map[string]any{
					"name":        t["name"],
					"description": t["description"],
					"parameters":  t["parameters"],
				},
			}
		}
	}
	return out
}

func executeToolInternal(ctx context.Context, name string, args map[string]any) (any, error) {
	switch name {
	case "memory_query":
		query := ""
		if v, ok := args["query"]; ok && v != nil {
			query = toString(v)
		}
		topK := 8
		if v, ok := args["top_k"]; ok && v != nil {
			if n, ok := toInt(v); ok {
				topK = n
			}
		}
		useReranker := true
		if v, ok := args["use_reranker"]; ok && v != nil {
			if b, ok := v.(bool); ok {
				useReranker = b
			}
		}
		body := map[string]any{
			"query":        query,
			"top_k":        topK,
			"use_reranker": useReranker,
			"raw":          false,
		}
		if v, ok := args["pack_uri"]; ok && v != nil {
			body["pack_uri"] = toString(v)
		}
		return clientPost(ctx, "/query", body)
	case "memory_status":
		return clientGet(ctx, "/status")
	case "memory_sources":
		result, err := clientGet(ctx, "/status")
		if err != nil {
			return nil, err
		}
		sources, _ := result["sources"].([]any)
		if sources == nil {
			sources = []any{}
		}
		return map[string]any{"sources": sources}, nil
	case "memory_add":
		body := map[string]any{}
		if docs, ok := args["documents"].([]any); ok && len(docs) > 0 {
			docList := make([]map[string]any, 0, len(docs))
			for _, d := range docs {
				s := toString(d)
				if strings.HasPrefix(s, "http://") || strings.HasPrefix(s, "https://") {
					docList = append(docList, map[string]any{"type": "url", "value": s})
				} else {
					docList = append(docList, map[string]any{"type": "content", "value": s})
				}
			}
			body["documents"] = docList
		}
		if conv, ok := args["conversation"]; ok && conv != nil {
			body["conversation"] = conv
		}
		_, err := clientPost(ctx, "/add", body)
		if err != nil {
			return nil, err
		}
		return map[string]any{"status": "ok"}, nil
	default:
		return nil, fmt.Errorf("memkit: unknown tool %s", name)
	}
}

func toString(v any) string {
	if v == nil {
		return ""
	}
	if s, ok := v.(string); ok {
		return s
	}
	return fmt.Sprintf("%v", v)
}

func toInt(v any) (int, bool) {
	switch x := v.(type) {
	case int:
		return x, true
	case int64:
		return int(x), true
	case float64:
		return int(x), true
	default:
		return 0, false
	}
}
