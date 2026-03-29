package sdk

// QueryOpts configures the Query call.
type QueryOpts struct {
	TopK        int    `json:"-"`
	PackURI     string `json:"pack_uri,omitempty"`
	UseReranker bool   `json:"use_reranker"`
	Raw         bool   `json:"raw"`
}

// ConversationMessage represents a single turn in a conversation.
type ConversationMessage struct {
	Role    string `json:"role"`
	Content string `json:"content"`
}
