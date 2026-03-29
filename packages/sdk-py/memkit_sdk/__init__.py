import json
import os
import re
from pathlib import Path

from .client import client_post, set_config
from .tools import execute_tool_internal, get_tools_for_provider


def configure(url: str | None = None) -> None:
    if url is not None:
        set_config(url)


def memkit(model: str) -> tuple[str, list]:
    provider = "openai" if (model.startswith("gpt-") or model.startswith("o1-")) else "anthropic"
    tools = get_tools_for_provider(provider)
    return (model, tools)


def query(
    text: str,
    *,
    pack_uri: str | None = None,
    top_k: int = 8,
    use_reranker: bool = True,
    raw: bool = False,
) -> dict:
    body = {
        "query": text,
        "top_k": top_k,
        "use_reranker": use_reranker,
        "raw": raw,
    }
    if pack_uri:
        body["pack_uri"] = pack_uri
    return client_post("/query", body)


def _resolve_document(s: str) -> dict:
    if s.startswith("http://") or s.startswith("https://"):
        return {"type": "url", "value": s}
    if s.startswith("~/") or s.startswith("/") or s.startswith("./") or re.match(r"^[A-Za-z]:[\\/]", s):
        expanded = os.path.expanduser(s) if s.startswith("~/") else s
        content = Path(expanded).read_text(encoding="utf-8")
        return {"type": "content", "value": content}
    return {"type": "content", "value": s}


def _normalize_add_input(items: str | list[str] | list[dict]) -> dict:
    if isinstance(items, str):
        return {"documents": [{"type": "content", "value": items}]}
    if isinstance(items, list) and len(items) > 0:
        first = items[0]
        if isinstance(first, str):
            documents = [_resolve_document(s) for s in items]
            return {"documents": documents}
        if isinstance(first, dict) and "role" in first and "content" in first:
            return {"conversation": items}
    raise ValueError("memkit.add: expected str, list[str], or list[dict] with role, content")


def add(items: str | list[str] | list[dict]) -> None:
    body = _normalize_add_input(items)
    client_post("/add", body)


def execute_tool(name: str, args: dict) -> str:
    result = execute_tool_internal(name, args)
    return result if isinstance(result, str) else json.dumps(result)


__all__ = ["configure", "memkit", "query", "add", "execute_tool"]
