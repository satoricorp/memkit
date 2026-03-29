export interface MemkitConfig {
    url: string;
}
export interface QueryOptions {
    pack_uri?: string;
    top_k?: number;
    raw?: boolean;
    use_reranker?: boolean;
}
export interface QueryResult {
    answer?: string;
    sources?: Array<{
        path: string;
        score: number;
    }>;
    provider?: string;
    results?: Array<{
        content: string;
        score: number;
        file_path: string;
        chunk_id: string;
        chunk_index: number;
    }>;
    timings_ms?: Record<string, number>;
}
export interface AddItem {
    type: "url" | "content";
    value: string;
}
export interface ConversationMessage {
    role: string;
    content: string;
}
//# sourceMappingURL=types.d.ts.map