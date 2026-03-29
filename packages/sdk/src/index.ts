import { clientGet, clientPost, getConfig, setConfig } from "./client.js";
import { getToolsForProvider, executeToolInternal } from "./tools.js";
import type { QueryOptions, QueryResult } from "./types.js";

export function configure(options: { url?: string }): void {
  if (options.url !== undefined) setConfig({ url: options.url });
}

export function memkit(model: string): { model: string; tools: unknown[] } {
  const provider =
    model.startsWith("gpt-") || model.startsWith("o1-") ? "openai" : "anthropic";
  const tools = getToolsForProvider(provider);
  return { model, tools };
}

export async function query(
  text: string,
  options?: QueryOptions
): Promise<QueryResult> {
  const body: Record<string, unknown> = {
    query: text,
    top_k: options?.top_k ?? 8,
    use_reranker: options?.use_reranker ?? true,
    raw: options?.raw ?? false,
  };
  if (options?.pack_uri) {
    body.pack_uri = options.pack_uri;
  }
  const result = (await clientPost("/query", body)) as QueryResult;
  return result;
}

export async function executeTool(
  name: string,
  args: Record<string, unknown>
): Promise<string> {
  const result = await executeToolInternal(name, args);
  return typeof result === "string" ? result : JSON.stringify(result);
}

export async function add(
  items:
    | string
    | string[]
    | Array<{ role: string; content: string }>
): Promise<void> {
  const body = await normalizeAddInput(items);
  await clientPost("/add", body);
}

async function normalizeAddInput(
  items: string | string[] | Array<{ role: string; content: string }>
): Promise<{
  documents?: Array<{ type: string; value: string }>;
  conversation?: Array<{ role: string; content: string }>;
}> {
  if (typeof items === "string") {
    return {
      documents: [{ type: "content", value: items }],
    };
  }
  if (Array.isArray(items) && items.length > 0) {
    const first = items[0];
    if (typeof first === "string") {
      const documents = await Promise.all(
        (items as string[]).map((s) => resolveDocument(s))
      );
      return { documents };
    }
    if (typeof first === "object" && "role" in first && "content" in first) {
      return { conversation: items as Array<{ role: string; content: string }> };
    }
  }
  throw new Error("memkit.add: expected string, string[], or { role, content }[]");
}

async function resolveDocument(
  s: string
): Promise<{ type: "url" | "content"; value: string }> {
  if (s.startsWith("http://") || s.startsWith("https://")) {
    return { type: "url", value: s };
  }
  if (
    s.startsWith("~/") ||
    s.startsWith("/") ||
    s.startsWith("./") ||
    /^[A-Za-z]:[\\/]/.test(s)
  ) {
    const fs = await import("fs/promises");
    const path = await import("path");
    const expanded = s.startsWith("~/")
      ? path.join(process.env.HOME ?? "", s.slice(2))
      : s;
    const content = await fs.readFile(expanded, "utf-8");
    return { type: "content", value: content };
  }
  return { type: "content", value: s };
}

const defaultExport = Object.assign(memkit, {
  configure,
  query,
  add,
  executeTool,
});

export default defaultExport;
