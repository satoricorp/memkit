"use node";

import { action, internalAction, type ActionCtx } from "./_generated/server";
import type { Id } from "./_generated/dataModel";
import { v } from "convex/values";
import { createHash, randomBytes } from "crypto";
import { SignJWT, importPKCS8 } from "jose";
import { makeFunctionReference, type FunctionReference } from "convex/server";

const LOGIN_GRANT_TTL_MS = 5 * 60 * 1000;
const JWT_TTL_MS = 15 * 60 * 1000;
const SESSION_TTL_MS = 30 * 24 * 60 * 60 * 1000;

type CliProfile = {
  user_id?: number | null;
  org_id?: number | null;
  auth_id?: string | null;
  name?: string | null;
  email?: string | null;
  image?: string | null;
  image_url?: string | null;
  created_at?: string | null;
  updated_at?: string | null;
};

type UserId = Id<"users">;
type CliLoginGrantId = Id<"cliLoginGrants">;
type CliSessionId = Id<"cliSessions">;

type AuthPayload = {
  sessionToken: string;
  jwt: string;
  jwtExpiresAt: string;
  profile: CliProfile;
};

type AuthError = {
  code: string;
  message: string;
};

type AuthActionResult =
  | {
    ok: true;
    auth: AuthPayload;
  }
  | {
    ok: false;
    error: AuthError;
  };

type LogoutActionResult = {
  ok: true;
  revoked: boolean;
};

type ConsumeLoginGrantResult =
  | {
    ok: true;
    user_id: UserId;
  }
  | {
    ok: false;
    reason: "not_found" | "used" | "expired";
  };

type TouchCliSessionResult =
  | {
    ok: true;
    user_id: UserId;
    session_id: CliSessionId;
  }
  | {
    ok: false;
    reason: "not_found" | "revoked" | "expired";
  };

type RevokeCliSessionResult =
  | { ok: true }
  | {
    ok: false;
    reason: "not_found";
  };

let privateKeyPromise: Promise<CryptoKey> | null = null;

function internalQueryRef<Args extends Record<string, any>, ReturnValue>(name: string) {
  return makeFunctionReference<"query", Args, ReturnValue>(name) as unknown as FunctionReference<
    "query",
    "internal",
    Args,
    ReturnValue
  >;
}

function internalMutationRef<Args extends Record<string, any>, ReturnValue>(name: string) {
  return makeFunctionReference<"mutation", Args, ReturnValue>(name) as unknown as FunctionReference<
    "mutation",
    "internal",
    Args,
    ReturnValue
  >;
}

const getUserProfileByIdRef = internalQueryRef<{ id: UserId }, CliProfile | null>(
  "users:getUserProfileById",
);
const createLoginGrantRef = internalMutationRef<
  {
    code_hash: string;
    user_id: UserId;
    expiration_time: number;
    created_at: string;
  },
  CliLoginGrantId
>("cli_auth_store:createLoginGrant");
const consumeLoginGrantRef = internalMutationRef<
  { code_hash: string; used_time: number },
  ConsumeLoginGrantResult
>("cli_auth_store:consumeLoginGrant");
const createCliSessionRef = internalMutationRef<
  {
    token_hash: string;
    user_id: UserId;
    expiration_time: number;
    last_used_time: number;
    created_at: string;
    updated_at: string;
  },
  CliSessionId
>("cli_auth_store:createCliSession");
const touchCliSessionRef = internalMutationRef<
  {
    token_hash: string;
    now: number;
    expiration_time: number;
    updated_at: string;
  },
  TouchCliSessionResult
>("cli_auth_store:touchCliSession");
const revokeCliSessionRef = internalMutationRef<
  {
    token_hash: string;
    revoked_time: number;
    updated_at: string;
  },
  RevokeCliSessionResult
>("cli_auth_store:revokeCliSession");

function requireEnv(name: string): string {
  const value = process.env[name];
  if (!value) {
    throw new Error(`${name} is not configured`);
  }
  return value;
}

function hashSecret(value: string) {
  return createHash("sha256").update(value).digest("hex");
}

function randomSecret(bytes: number) {
  return randomBytes(bytes).toString("base64url");
}

async function signingKey() {
  if (!privateKeyPromise) {
    privateKeyPromise = importPKCS8(requireEnv("JWT_PRIVATE_KEY"), "RS256");
  }
  return privateKeyPromise;
}

async function loadProfile(ctx: ActionCtx, userId: UserId): Promise<CliProfile> {
  const profile = await ctx.runQuery(getUserProfileByIdRef, {
    id: userId,
  });
  if (!profile) {
    throw new Error("Authenticated user not found");
  }
  return profile;
}

async function signCliJwt(
  userId: UserId,
  sessionId: CliSessionId,
  profile: CliProfile,
) {
  const expiresAt = new Date(Date.now() + JWT_TTL_MS);
  const claims: Record<string, string> = {
    sub: `${userId}|${sessionId}`,
  };
  if (profile.name) {
    claims.name = profile.name;
  }
  if (profile.email) {
    claims.email = profile.email;
  }
  if (profile.image_url || profile.image) {
    claims.picture = profile.image_url ?? profile.image ?? "";
  }
  const jwt = await new SignJWT(claims)
    .setProtectedHeader({ alg: "RS256" })
    .setIssuedAt()
    .setIssuer(requireEnv("CONVEX_SITE_URL"))
    .setAudience("convex")
    .setExpirationTime(expiresAt)
    .sign(await signingKey());
  return {
    jwt,
    jwtExpiresAt: expiresAt.toISOString(),
  };
}

async function buildAuthPayload(
  ctx: ActionCtx,
  userId: UserId,
  sessionId: CliSessionId,
  sessionToken: string,
): Promise<AuthPayload> {
  const profile = await loadProfile(ctx, userId);
  const signed = await signCliJwt(userId, sessionId, profile);
  return {
    sessionToken,
    jwt: signed.jwt,
    jwtExpiresAt: signed.jwtExpiresAt,
    profile,
  };
}

export const issueLoginGrant = internalAction({
  args: {
    user_id: v.id("users"),
  },
  handler: async (ctx, args): Promise<{ code: string }> => {
    const code = randomSecret(24);
    await ctx.runMutation(createLoginGrantRef, {
      code_hash: hashSecret(code),
      user_id: args.user_id,
      expiration_time: Date.now() + LOGIN_GRANT_TTL_MS,
      created_at: new Date().toISOString(),
    });
    return { code };
  },
});

export const exchangeLoginGrant = action({
  args: {
    code: v.string(),
  },
  handler: async (ctx, args): Promise<AuthActionResult> => {
    const now = Date.now();
    const consumed = await ctx.runMutation(consumeLoginGrantRef, {
      code_hash: hashSecret(args.code),
      used_time: now,
    });

    if (!consumed.ok) {
      return {
        ok: false as const,
        error: {
          code: "INVALID_LOGIN_CODE",
          message: "Login code is invalid or expired.",
        },
      };
    }

    const sessionToken = randomSecret(32);
    const sessionId = await ctx.runMutation(createCliSessionRef, {
      token_hash: hashSecret(sessionToken),
      user_id: consumed.user_id,
      expiration_time: now + SESSION_TTL_MS,
      last_used_time: now,
      created_at: new Date(now).toISOString(),
      updated_at: new Date(now).toISOString(),
    });

    return {
      ok: true as const,
      auth: await buildAuthPayload(ctx, consumed.user_id, sessionId, sessionToken),
    };
  },
});

export const refreshCliSession = action({
  args: {
    sessionToken: v.string(),
  },
  handler: async (ctx, args): Promise<AuthActionResult> => {
    const now = Date.now();
    const touched = await ctx.runMutation(touchCliSessionRef, {
      token_hash: hashSecret(args.sessionToken),
      now,
      expiration_time: now + SESSION_TTL_MS,
      updated_at: new Date(now).toISOString(),
    });

    if (!touched.ok) {
      return {
        ok: false as const,
        error: {
          code: "INVALID_SESSION",
          message: "CLI session is invalid or expired.",
        },
      };
    }

    return {
      ok: true as const,
      auth: await buildAuthPayload(
        ctx,
        touched.user_id,
        touched.session_id,
        args.sessionToken,
      ),
    };
  },
});

export const logoutCliSession = action({
  args: {
    sessionToken: v.string(),
  },
  handler: async (ctx, args): Promise<LogoutActionResult> => {
    const revoked = await ctx.runMutation(revokeCliSessionRef, {
      token_hash: hashSecret(args.sessionToken),
      revoked_time: Date.now(),
      updated_at: new Date().toISOString(),
    });

    return {
      ok: true as const,
      revoked: revoked.ok,
    };
  },
});
