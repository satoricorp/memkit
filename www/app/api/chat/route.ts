'use server'

import type OpenAI from "openai";

type ChatMessage = {
  role: "user" | "assistant" | "system";
  content: string;
};

type CompletionResponse = {
  choices?: Array<{
    message?: { content?: string };
  }>;
  usage?: {
    prompt_tokens?: number;
    completion_tokens?: number;
  } | null;
  metadata?: {
    usage?: {
      input_tokens?: number;
      output_tokens?: number;
      total_tokens?: number;
    };
  };
  completion_tokens?: number;
};

const apiKey = process.env.AI_API_KEY;
const baseURL = process.env.AI_BASE_URL ?? "https://api.openai.com/v1";
const model = process.env.AI_MODEL ?? "gpt-4o-mini";

export async function POST(request: Request) {
  try {
    if (!apiKey) {
      console.error("Missing AI_API_KEY");
      return Response.json({ error: "Missing API key" }, { status: 500 });
    }
    const { messages } = (await request.json()) as { messages?: ChatMessage[] };

    if (!messages || !Array.isArray(messages)) {
      return Response.json({ error: "Messages are required" }, { status: 400 });
    }

    const params: OpenAI.ChatCompletionCreateParamsNonStreaming = {
      model,
      messages,
      temperature: 0.6,
    };
    const response = await fetch(`${baseURL}/chat/completions`, {
      method: "POST",
      headers: {
        Authorization: `Bearer ${apiKey}`,
        "Content-Type": "application/json",
      },
      body: JSON.stringify(params),
    });

    if (!response.ok) {
      const errorText = await response.text();
      console.error("AI API error", response.status, errorText);
      return Response.json(
        { error: "Chat request failed" },
        { status: response.status },
      );
    }

    const completion = (await response.json()) as CompletionResponse;
    const message = completion.choices?.[0]?.message?.content ?? "";
    const inputTokens =
      completion.metadata?.usage?.input_tokens ?? completion.usage?.prompt_tokens ?? 0;
    const outputTokens =
      completion.completion_tokens ??
      completion.metadata?.usage?.output_tokens ??
      completion.usage?.completion_tokens ??
      0;

    return Response.json({
      message,
      usage: {
        actualPromptTokens: inputTokens,
        baselinePromptTokens: 0,
        outputTokens,
      },
    });
  } catch (error) {
    console.error("Chat API error:", error);
    return Response.json({ error: "Failed to generate response" }, { status: 500 });
  }
}
