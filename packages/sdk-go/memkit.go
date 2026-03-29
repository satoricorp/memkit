package sdk

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"strings"
)

// Configure sets the memkit daemon URL. Empty string is a no-op.
func Configure(url string) {
	setConfigURL(url)
}

// Memkit returns the model and tools for the given model name.
// Provider is inferred: gpt-* or o1-* → openai, else → anthropic.
func Memkit(model string) (string, []any) {
	provider := "anthropic"
	if strings.HasPrefix(model, "gpt-") || strings.HasPrefix(model, "o1-") {
		provider = "openai"
	}
	tools := GetToolsForProvider(provider)
	return model, tools
}

// Query runs a semantic search against the memory pack.
func Query(ctx context.Context, text string, opts *QueryOpts) (map[string]any, error) {
	topK := 8
	useReranker := true
	raw := false
	if opts != nil {
		if opts.TopK > 0 {
			topK = opts.TopK
		}
		useReranker = opts.UseReranker
		raw = opts.Raw
	}
	body := map[string]any{
		"query":        text,
		"top_k":        topK,
		"use_reranker": useReranker,
		"raw":          raw,
	}
	if opts != nil && opts.PackURI != "" {
		body["pack_uri"] = opts.PackURI
	}
	return clientPost(ctx, "/query", body)
}

// Add ingests documents or conversation into the memory pack.
// items may be: string, []string, or []ConversationMessage.
func Add(ctx context.Context, items any) error {
	body, err := normalizeAddInput(items)
	if err != nil {
		return err
	}
	_, err = clientPost(ctx, "/add", body)
	return err
}

func normalizeAddInput(items any) (map[string]any, error) {
	switch v := items.(type) {
	case string:
		return map[string]any{
			"documents": []map[string]any{{"type": "content", "value": v}},
		}, nil
	case []string:
		if len(v) == 0 {
			return nil, fmt.Errorf("memkit.add: expected string, []string, or []ConversationMessage")
		}
		docs := make([]map[string]any, 0, len(v))
		for _, s := range v {
			d, err := resolveDocument(s)
			if err != nil {
				return nil, err
			}
			docs = append(docs, d)
		}
		return map[string]any{"documents": docs}, nil
	case []ConversationMessage:
		if len(v) == 0 {
			return nil, fmt.Errorf("memkit.add: expected string, []string, or []ConversationMessage")
		}
		conv := make([]map[string]any, len(v))
		for i, m := range v {
			conv[i] = map[string]any{"role": m.Role, "content": m.Content}
		}
		return map[string]any{"conversation": conv}, nil
	default:
		return nil, fmt.Errorf("memkit.add: expected string, []string, or []ConversationMessage")
	}
}

var winDriveRegex = regexp.MustCompile(`^[A-Za-z]:[\\/]`)

func resolveDocument(s string) (map[string]any, error) {
	if strings.HasPrefix(s, "http://") || strings.HasPrefix(s, "https://") {
		return map[string]any{"type": "url", "value": s}, nil
	}
	if strings.HasPrefix(s, "~/") || strings.HasPrefix(s, "/") || strings.HasPrefix(s, "./") || winDriveRegex.MatchString(s) {
		path := s
		if strings.HasPrefix(s, "~/") {
			home, err := os.UserHomeDir()
			if err != nil {
				return nil, err
			}
			path = filepath.Join(home, s[2:])
		}
		content, err := os.ReadFile(path)
		if err != nil {
			return nil, err
		}
		return map[string]any{"type": "content", "value": string(content)}, nil
	}
	return map[string]any{"type": "content", "value": s}, nil
}

// ExecuteTool runs a tool by name with the given args and returns the result as a JSON string.
func ExecuteTool(ctx context.Context, name string, args map[string]any) (string, error) {
	result, err := executeToolInternal(ctx, name, args)
	if err != nil {
		return "", err
	}
	if s, ok := result.(string); ok {
		return s, nil
	}
	b, err := json.Marshal(result)
	if err != nil {
		return "", err
	}
	return string(b), nil
}
