import { mutation, query } from "./_generated/server";
import { v } from "convex/values";

const orgArgs = {
  org_id: v.number(),
  name: v.optional(v.string()),
  email: v.optional(v.string()),
  api_key: v.optional(v.string()),
  api_key_hash: v.string(),
  polar_customer_id: v.optional(v.union(v.string(), v.null())),
  polar_subscription_id: v.optional(v.union(v.string(), v.null())),
  created_at: v.optional(v.string()),
  updated_at: v.optional(v.string()),
};

export const getOrgIdByApiKeyHash = query({
  args: { api_key_hash: v.string() },
  handler: async (ctx, args) => {
    const org = await ctx.db
      .query("orgs")
      .withIndex("by_api_key_hash", (q) => q.eq("api_key_hash", args.api_key_hash))
      .unique();
    return org?.org_id ?? null;
  },
});

export const getOrgIdByEmail = query({
  args: { email: v.string() },
  handler: async (ctx, args) => {
    const email = args.email.trim().toLowerCase();
    const orgs = await ctx.db.query("orgs").collect();
    const matches = orgs.filter(
      (org) => (org.email ?? "").trim().toLowerCase() === email,
    );
    if (matches.length === 0) {
      return null;
    }
    const latest = matches.reduce((best, org) => {
      const bestStamp = (best.updated_at ?? best.created_at ?? "").toString();
      const orgStamp = (org.updated_at ?? org.created_at ?? "").toString();
      return orgStamp > bestStamp ? org : best;
    });
    return latest.org_id ?? null;
  },
});

export const getOrgById = query({
  args: { org_id: v.number() },
  handler: async (ctx, args) => {
    const org = await ctx.db
      .query("orgs")
      .withIndex("by_org_id", (q) => q.eq("org_id", args.org_id))
      .unique();
    if (!org) {
      return null;
    }
    return {
      org_id: org.org_id,
      api_key: org.api_key ?? null,
      api_key_hash: org.api_key_hash,
      name: org.name ?? null,
      email: org.email ?? null,
      polar_customer_id: org.polar_customer_id ?? null,
      polar_subscription_id: org.polar_subscription_id ?? null,
      created_at: org.created_at ?? null,
      updated_at: org.updated_at ?? null,
    };
  },
});

export const getPolarCredsByAuthId = query({
  args: { auth_id: v.string() },
  handler: async (ctx, args) => {
    const user = await ctx.db
      .query("users")
      .withIndex("by_auth_id", (q) => q.eq("auth_id", args.auth_id))
      .unique();
    if (!user?.org_id) {
      return { hasPolarCreds: false };
    }
    const orgId = user.org_id;
    const org = await ctx.db
      .query("orgs")
      .withIndex("by_org_id", (q) => q.eq("org_id", orgId))
      .unique();
    if (!org) {
      return { hasPolarCreds: false };
    }
    const hasPolarCreds = Boolean(
      org.polar_subscription_id || org.polar_customer_id,
    );
    return {
      hasPolarCreds,
      polar_customer_id: org.polar_customer_id ?? null,
      polar_subscription_id: org.polar_subscription_id ?? null,
    };
  },
});

export const getPolarCredsForViewer = query({
  args: {},
  handler: async (ctx) => {
    const identity = await ctx.auth.getUserIdentity();
    if (!identity) {
      return { hasPolarCreds: false };
    }
    const user = await ctx.db
      .query("users")
      .withIndex("by_auth_id", (q) => q.eq("auth_id", identity.subject))
      .unique();
    if (!user?.org_id) {
      return { hasPolarCreds: false };
    }
    const orgId = user.org_id;
    const org = await ctx.db
      .query("orgs")
      .withIndex("by_org_id", (q) => q.eq("org_id", orgId))
      .unique();
    if (!org) {
      return { hasPolarCreds: false };
    }
    const hasPolarCreds = Boolean(
      org.polar_subscription_id || org.polar_customer_id,
    );
    return {
      hasPolarCreds,
      polar_customer_id: org.polar_customer_id ?? null,
      polar_subscription_id: org.polar_subscription_id ?? null,
    };
  },
});

export const upsertOrg = mutation({
  args: orgArgs,
  handler: async (ctx, args) => {
    const existing = await ctx.db
      .query("orgs")
      .withIndex("by_org_id", (q) => q.eq("org_id", args.org_id))
      .unique();
    const updates = Object.fromEntries(
      Object.entries(args).filter(([, value]) => value !== undefined),
    );
    if (existing) {
      await ctx.db.patch(existing._id, updates);
      return existing._id;
    }
    return await ctx.db.insert("orgs", args);
  },
});
