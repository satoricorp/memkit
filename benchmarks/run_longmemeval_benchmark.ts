import { mkdirSync, rmSync, appendFileSync, writeFileSync } from "fs";
import { join } from "path";

const MEMKIT_SERVER = process.env.MEMKIT_SERVER || "http://127.0.0.1:4242";
const DATA_DIR = "./data";
const RUN_ID = process.env.BENCHMARK_RUN_ID || `${Date.now()}`;
const PACK_PATH = process.env.PACK_PATH || `/tmp/longmemeval-${RUN_ID}`;
const OUTPUT_DIR = process.env.OUTPUT_DIR || "./output";
const DEFAULT_OUTPUT_FILE = join(OUTPUT_DIR, `longmemeval_${RUN_ID}.jsonl`);
const DEFAULT_LOG_FILE = join(OUTPUT_DIR, `benchmark_${RUN_ID}.log`);
const OUTPUT_FILE = process.env.OUTPUT_FILE || DEFAULT_OUTPUT_FILE;
const LOG_FILE = process.env.LOG_FILE || DEFAULT_LOG_FILE;
const MAX_QUESTIONS = Number(process.env.MAX_QUESTIONS || 50);
const TOP_K = Number(process.env.TOP_K || 10);
const USE_RERANKER = process.env.USE_RERANKER !== "0";
const BATCH_SIZE = Number(process.env.BATCH_SIZE || 32);
const EMBED_PROVIDER = process.env.EMBED_PROVIDER || "fastembed";
const EMBED_MODEL = process.env.EMBED_MODEL || (EMBED_PROVIDER === "hash" ? "hash" : "BAAI/bge-small-en-v1.5");
const EMBED_DIM = Number(process.env.EMBED_DIM || 384);
const CONVERSATION_PROVIDER =
  process.env.MEMKIT_CONVERSATION_PROVIDER || (process.env.OPENAI_API_KEY ? "openai" : "llama");
const CONVERSATION_MODEL = process.env.MEMKIT_CONVERSATION_MODEL || "";
const ADD_TIMEOUT_MS = Number(process.env.ADD_TIMEOUT_MS || 30 * 60 * 1000);
const QUERY_TIMEOUT_MS = Number(process.env.QUERY_TIMEOUT_MS || 5 * 60 * 1000);
const ADD_RETRY_LIMIT = Number(process.env.ADD_RETRY_LIMIT || 4);
const ADD_RETRY_BACKOFF_MS = Number(process.env.ADD_RETRY_BACKOFF_MS || 1500);
const PROGRESS_FILE = process.env.PROGRESS_FILE || `${OUTPUT_FILE}.progress.json`;

interface SessionTurn {
  role: string;
  content: string;
}

interface QuestionData {
  question_id: string;
  question: string;
  answer: string;
  haystack_dates: string[];
  haystack_session_ids: string[];
  haystack_sessions: SessionTurn[][];
}

interface BenchmarkHit {
  content?: string;
  score?: number;
  file_path?: string;
  memory?: {
    record_type?: string;
    relation_kind?: string;
    entity_kind?: string;
    value_kind?: string;
  };
}

function log(msg: string) {
  const line = `[${new Date().toISOString()}] ${msg}`;
  appendFileSync(LOG_FILE, line + "\n");
  console.log(msg);
}

function sleep(ms: number) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function writeProgress(progress: Record<string, unknown>) {
  writeFileSync(PROGRESS_FILE, JSON.stringify(progress, null, 2));
}

async function fetchWithTimeout(url: string, init: RequestInit, timeoutMs: number): Promise<Response> {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(`timeout after ${timeoutMs}ms`), timeoutMs);
  try {
    return await fetch(url, { ...init, signal: controller.signal });
  } finally {
    clearTimeout(timer);
  }
}

function checkAnswer(answer: string, prediction: string): boolean {
  if (!answer || !prediction) return false;
  const a = String(answer).toLowerCase();
  const p = String(prediction).toLowerCase();
  if (p.includes(a)) return true;
  return a.split(" ").filter((w: string) => w.length > 4).some((w: string) => p.includes(w));
}

function bootstrapPack() {
  mkdirSync(join(PACK_PATH, ".memkit", "state"), { recursive: true });
  const manifest = {
    format_version: "1.0.0",
    pack_id: "longmemeval-benchmark",
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
    embedding: {
      provider: EMBED_PROVIDER,
      model: EMBED_MODEL,
      dimension: EMBED_DIM,
    },
    chunking: {
      strategy: "char_window",
      target_chars: 1200,
      overlap_chars: 200,
    },
    conversation: {
      strategy: "dual_timestamp_memory",
      extraction_provider: CONVERSATION_PROVIDER,
      hydrate_evidence: true,
    },
    sources: [],
  };
  writeFileSync(join(PACK_PATH, ".memkit", "manifest.json"), JSON.stringify(manifest, null, 2));
  writeFileSync(join(PACK_PATH, ".memkit", "state", "file_state.json"), "[]");
}

async function main() {
  const data: QuestionData[] = JSON.parse(
    require("fs").readFileSync(join(DATA_DIR, "longmemeval_s_cleaned.json"), "utf-8")
  );

  console.log("=== Creating pack and indexing sessions for first", MAX_QUESTIONS, "questions ===\n");

  rmSync(PACK_PATH, { recursive: true, force: true });
  mkdirSync(PACK_PATH, { recursive: true });
  mkdirSync(OUTPUT_DIR, { recursive: true });
  rmSync(OUTPUT_FILE, { force: true });
  rmSync(LOG_FILE, { force: true });
  rmSync(PROGRESS_FILE, { force: true });
  bootstrapPack();

  const indexedSessionIds = new Set<string>();
  let totalSessionAdds = 0;
  let queuedSessions: { session_id: string; session_time?: string; conversation: SessionTurn[] }[] = [];
  const indexingStart = Date.now();

  log(
    `Indexing sessions for first ${MAX_QUESTIONS} questions (pack=${PACK_PATH}, top_k=${TOP_K}, use_reranker=${USE_RERANKER}, batch_size=${BATCH_SIZE}, embed_provider=${EMBED_PROVIDER}, embed_model=${EMBED_MODEL}, embed_dim=${EMBED_DIM}, conversation_provider=${CONVERSATION_PROVIDER}, conversation_model=${CONVERSATION_MODEL || "default"}, add_timeout_ms=${ADD_TIMEOUT_MS}, query_timeout_ms=${QUERY_TIMEOUT_MS})...`
  );

  async function postConversationBatch(
    batch: { session_id: string; session_time?: string; conversation: SessionTurn[] }[],
    attempt: number
  ): Promise<void> {
    const sessionIds = batch.map((session) => session.session_id || "<missing-session-id>");
    try {
      const res = await fetchWithTimeout(
        `${MEMKIT_SERVER}/add`,
        {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ pack: PACK_PATH, conversations: batch }),
        },
        ADD_TIMEOUT_MS
      );
      if (!res.ok) {
        const body = await res.text();
        throw new Error(
          `HTTP ${res.status} for batch(size=${batch.length}, attempt=${attempt}, sessions=${sessionIds.join(",")}): ${body || "<empty body>"}`
        );
      }
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      throw new Error(
        `batch(size=${batch.length}, attempt=${attempt}, sessions=${sessionIds.join(",")}): ${message}`
      );
    }
  }

  async function flushBatchWithRecovery(
    batch: { session_id: string; session_time?: string; conversation: SessionTurn[] }[],
    depth = 0
  ): Promise<void> {
    if (batch.length === 0) return;
    let lastError: Error | null = null;
    for (let attempt = 1; attempt <= ADD_RETRY_LIMIT; attempt++) {
      try {
        await postConversationBatch(batch, attempt);
        if (attempt > 1) {
          log(`Recovered add batch after retry ${attempt} (size=${batch.length}, depth=${depth})`);
        }
        return;
      } catch (error) {
        lastError = error instanceof Error ? error : new Error(String(error));
        log(`Add batch failed (attempt ${attempt}/${ADD_RETRY_LIMIT}, size=${batch.length}, depth=${depth}): ${lastError.message}`);
        if (attempt < ADD_RETRY_LIMIT) {
          await sleep(ADD_RETRY_BACKOFF_MS * attempt);
        }
      }
    }

    if (batch.length > 1) {
      const midpoint = Math.ceil(batch.length / 2);
      log(`Splitting failed batch (size=${batch.length}, depth=${depth}) into ${midpoint} and ${batch.length - midpoint}`);
      await flushBatchWithRecovery(batch.slice(0, midpoint), depth + 1);
      await flushBatchWithRecovery(batch.slice(midpoint), depth + 1);
      return;
    }

    throw new Error(`Unrecoverable single-session add failure: ${lastError?.message || "unknown add failure"}`);
  }

  async function flushBatch() {
    if (queuedSessions.length === 0) return;
    const batch = queuedSessions;
    queuedSessions = [];
    await flushBatchWithRecovery(batch);
  }

  // Step 1: Index sessions for the questions we'll test
  for (let i = 0; i < MAX_QUESTIONS; i++) {
    const question = data[i];
    for (let s = 0; s < question.haystack_sessions.length; s++) {
      const session = question.haystack_sessions[s];
      const sessionId = question.haystack_session_ids[s];
      const sessionTime = question.haystack_dates[s];

      if (!indexedSessionIds.has(sessionId)) {
        queuedSessions.push({
          session_id: sessionId,
          session_time: sessionTime,
          conversation: session,
        });
        if (queuedSessions.length >= BATCH_SIZE) {
          await flushBatch();
        }
        indexedSessionIds.add(sessionId);
        totalSessionAdds++;
      }
    }
    log(`Indexed Q${i + 1}: ${totalSessionAdds} total sessions`);
    writeProgress({
      phase: "indexing",
      run_id: RUN_ID,
      pack_path: PACK_PATH,
      output_file: OUTPUT_FILE,
      log_file: LOG_FILE,
      questions_indexed: i + 1,
      total_sessions_indexed: totalSessionAdds,
      max_questions: MAX_QUESTIONS,
      use_reranker: USE_RERANKER,
      embed_provider: EMBED_PROVIDER,
      conversation_provider: CONVERSATION_PROVIDER,
    });
  }
  await flushBatch();

  const indexingElapsedMs = Date.now() - indexingStart;
  log(`Indexing complete: ${totalSessionAdds} sessions indexed`);
  log(`Indexing elapsed: ${(indexingElapsedMs / 1000).toFixed(1)}s`);
  writeProgress({
    phase: "querying",
    run_id: RUN_ID,
    pack_path: PACK_PATH,
    output_file: OUTPUT_FILE,
    log_file: LOG_FILE,
    questions_indexed: MAX_QUESTIONS,
    total_sessions_indexed: totalSessionAdds,
    max_questions: MAX_QUESTIONS,
    use_reranker: USE_RERANKER,
    embed_provider: EMBED_PROVIDER,
    conversation_provider: CONVERSATION_PROVIDER,
    indexing_elapsed_ms: indexingElapsedMs,
  });
  console.log("\n=== Running benchmark ===\n");

  let correct = 0;
  const queryStart = Date.now();

  for (let i = 0; i < MAX_QUESTIONS; i++) {
    const question = data[i];

    const res = await fetchWithTimeout(`${MEMKIT_SERVER}/query`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        pack: PACK_PATH,
        query: question.question,
        top_k: TOP_K,
        use_reranker: USE_RERANKER,
      }),
    }, QUERY_TIMEOUT_MS);
    const qData = await res.json();
    const answer = qData.answer || "";
    const resolvedPackPath = qData.resolved_pack_path || null;
    const topRetrievalHits = Array.isArray(qData.retrieval_results)
      ? (qData.retrieval_results as BenchmarkHit[]).slice(0, 3).map((hit) => ({
          score: hit.score,
          file_path: hit.file_path,
          content: hit.content,
          record_type: hit.memory?.record_type,
          relation_kind: hit.memory?.relation_kind,
          entity_kind: hit.memory?.entity_kind,
          value_kind: hit.memory?.value_kind,
        }))
      : [];
    const topFinalHits = Array.isArray(qData.results)
      ? (qData.results as BenchmarkHit[]).slice(0, 3).map((hit) => ({
          score: hit.score,
          file_path: hit.file_path,
          content: hit.content,
          record_type: hit.memory?.record_type,
          relation_kind: hit.memory?.relation_kind,
          entity_kind: hit.memory?.entity_kind,
          value_kind: hit.memory?.value_kind,
        }))
      : [];

    const isCorrect = checkAnswer(String(question.answer), answer);
    if (isCorrect) correct++;

    const expectedPackLeaf = PACK_PATH.split("/").filter(Boolean).pop();
    if (resolvedPackPath && expectedPackLeaf && !String(resolvedPackPath).includes(expectedPackLeaf)) {
      log(`Pack mismatch for ${question.question_id}: expected ${PACK_PATH}, got ${resolvedPackPath}`);
    }

    appendFileSync(
      OUTPUT_FILE,
      JSON.stringify({
        run_id: RUN_ID,
        run_config: {
          max_questions: MAX_QUESTIONS,
          top_k: TOP_K,
          use_reranker: USE_RERANKER,
          batch_size: BATCH_SIZE,
          embed_provider: EMBED_PROVIDER,
          embed_model: EMBED_MODEL,
          embed_dim: EMBED_DIM,
          conversation_provider: CONVERSATION_PROVIDER,
          conversation_model: CONVERSATION_MODEL || null,
          add_timeout_ms: ADD_TIMEOUT_MS,
          query_timeout_ms: QUERY_TIMEOUT_MS,
          memkit_server: MEMKIT_SERVER,
        },
        question_id: question.question_id,
        question: question.question,
        gold_answer: question.answer,
        hypothesis: answer,
        correct: isCorrect,
        resolved_pack_path: resolvedPackPath,
        top_retrieval_hits: topRetrievalHits,
        top_final_hits: topFinalHits,
      }) + "\n"
    );

    const elapsed = ((Date.now() - queryStart) / 1000).toFixed(1);
    log(`[${i + 1}/${MAX_QUESTIONS}] ${elapsed}s, ${correct} correct (${(correct / (i + 1)) * 100}%)`);
    writeProgress({
      phase: "querying",
      run_id: RUN_ID,
      pack_path: PACK_PATH,
      output_file: OUTPUT_FILE,
      log_file: LOG_FILE,
      questions_indexed: MAX_QUESTIONS,
      total_sessions_indexed: totalSessionAdds,
      questions_answered: i + 1,
      correct,
      max_questions: MAX_QUESTIONS,
      use_reranker: USE_RERANKER,
      embed_provider: EMBED_PROVIDER,
      conversation_provider: CONVERSATION_PROVIDER,
      indexing_elapsed_ms: indexingElapsedMs,
      query_elapsed_ms: Date.now() - queryStart,
    });
  }

  const queryElapsedMs = Date.now() - queryStart;
  const totalTime = ((indexingElapsedMs + queryElapsedMs) / 1000).toFixed(1);
  console.log(`\n=== RESULTS ===`);
  console.log(`Accuracy: ${(correct / MAX_QUESTIONS * 100).toFixed(1)}% (${correct}/${MAX_QUESTIONS})`);
  console.log(`Reranker: ${USE_RERANKER ? "on" : "off"}`);
  console.log(`Indexing time: ${(indexingElapsedMs / 1000).toFixed(1)}s`);
  console.log(`Query time: ${(queryElapsedMs / 1000).toFixed(1)}s`);
  console.log(`Total time: ${totalTime}s`);
  console.log(`Output saved to: ${OUTPUT_FILE}`);
  console.log(`Log saved to: ${LOG_FILE}`);
  writeProgress({
    phase: "complete",
    run_id: RUN_ID,
    pack_path: PACK_PATH,
    output_file: OUTPUT_FILE,
    log_file: LOG_FILE,
    questions_indexed: MAX_QUESTIONS,
    total_sessions_indexed: totalSessionAdds,
    questions_answered: MAX_QUESTIONS,
    correct,
    accuracy: correct / MAX_QUESTIONS,
    max_questions: MAX_QUESTIONS,
    use_reranker: USE_RERANKER,
    embed_provider: EMBED_PROVIDER,
    conversation_provider: CONVERSATION_PROVIDER,
    indexing_elapsed_ms: indexingElapsedMs,
    query_elapsed_ms: queryElapsedMs,
    total_elapsed_ms: indexingElapsedMs + queryElapsedMs,
  });
}

main().catch((error) => {
  const message = error instanceof Error ? `${error.stack || error.message}` : String(error);
  log(`FATAL: ${message}`);
  writeProgress({
    phase: "failed",
    run_id: RUN_ID,
    pack_path: PACK_PATH,
    output_file: OUTPUT_FILE,
    log_file: LOG_FILE,
    error: message,
    max_questions: MAX_QUESTIONS,
    use_reranker: USE_RERANKER,
    embed_provider: EMBED_PROVIDER,
    conversation_provider: CONVERSATION_PROVIDER,
  });
  console.error(error);
  process.exitCode = 1;
});
