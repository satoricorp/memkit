import { internalMutation } from "./_generated/server";
import { v } from "convex/values";

export const createLoginGrant = internalMutation({
  args: {
    code_hash: v.string(),
    user_id: v.id("users"),
    expiration_time: v.number(),
    created_at: v.string(),
  },
  handler: async (ctx, args) => {
    return await ctx.db.insert("cliLoginGrants", {
      code_hash: args.code_hash,
      user_id: args.user_id,
      expiration_time: args.expiration_time,
      created_at: args.created_at,
    });
  },
});

export const consumeLoginGrant = internalMutation({
  args: {
    code_hash: v.string(),
    used_time: v.number(),
  },
  handler: async (ctx, args) => {
    const grant = await ctx.db
      .query("cliLoginGrants")
      .withIndex("by_code_hash", (q) => q.eq("code_hash", args.code_hash))
      .unique();

    if (!grant) {
      return { ok: false as const, reason: "not_found" as const };
    }
    if (grant.used_time !== undefined) {
      return { ok: false as const, reason: "used" as const };
    }
    if (grant.expiration_time <= args.used_time) {
      return { ok: false as const, reason: "expired" as const };
    }

    await ctx.db.patch(grant._id, { used_time: args.used_time });
    return {
      ok: true as const,
      user_id: grant.user_id,
    };
  },
});

export const createCliSession = internalMutation({
  args: {
    token_hash: v.string(),
    user_id: v.id("users"),
    expiration_time: v.number(),
    last_used_time: v.number(),
    created_at: v.string(),
    updated_at: v.string(),
  },
  handler: async (ctx, args) => {
    return await ctx.db.insert("cliSessions", {
      token_hash: args.token_hash,
      user_id: args.user_id,
      expiration_time: args.expiration_time,
      last_used_time: args.last_used_time,
      created_at: args.created_at,
      updated_at: args.updated_at,
    });
  },
});

export const touchCliSession = internalMutation({
  args: {
    token_hash: v.string(),
    now: v.number(),
    expiration_time: v.number(),
    updated_at: v.string(),
  },
  handler: async (ctx, args) => {
    const session = await ctx.db
      .query("cliSessions")
      .withIndex("by_token_hash", (q) => q.eq("token_hash", args.token_hash))
      .unique();

    if (!session) {
      return { ok: false as const, reason: "not_found" as const };
    }
    if (session.revoked_time !== undefined) {
      return { ok: false as const, reason: "revoked" as const };
    }
    if (session.expiration_time <= args.now) {
      return { ok: false as const, reason: "expired" as const };
    }

    await ctx.db.patch(session._id, {
      last_used_time: args.now,
      expiration_time: args.expiration_time,
      updated_at: args.updated_at,
    });

    return {
      ok: true as const,
      user_id: session.user_id,
      session_id: session._id,
    };
  },
});

export const revokeCliSession = internalMutation({
  args: {
    token_hash: v.string(),
    revoked_time: v.number(),
    updated_at: v.string(),
  },
  handler: async (ctx, args) => {
    const session = await ctx.db
      .query("cliSessions")
      .withIndex("by_token_hash", (q) => q.eq("token_hash", args.token_hash))
      .unique();

    if (!session) {
      return { ok: false as const, reason: "not_found" as const };
    }

    await ctx.db.patch(session._id, {
      revoked_time: args.revoked_time,
      updated_at: args.updated_at,
    });

    return { ok: true as const };
  },
});
