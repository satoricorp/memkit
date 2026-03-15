import { mutation, query } from "./_generated/server";
import { getAuthUserId } from "@convex-dev/auth/server";
import { v } from "convex/values";

const userArgs = {
  id: v.optional(v.id("users")),
  user_id: v.optional(v.number()),
  org_id: v.optional(v.number()),
  auth_id: v.optional(v.string()),
  name: v.optional(v.string()),
  email: v.optional(v.string()),
  admin: v.optional(v.boolean()),
  first_name: v.optional(v.string()),
  last_name: v.optional(v.string()),
  username: v.optional(v.string()),
  image_url: v.optional(v.string()),
  created_at: v.optional(v.string()),
  updated_at: v.optional(v.string()),
};


export const upsertUser = mutation({
  args: userArgs,
  handler: async (ctx, args) => {
    const { id, ...rest } = args;
    const existing = args.user_id === undefined
      ? null
      : await ctx.db
        .query("users")
        .withIndex("by_user_id", (q) => q.eq("user_id", args.user_id))
        .unique();
    const updates = Object.fromEntries(
      Object.entries(rest).filter(([, value]) => value !== undefined),
    ) as Partial<typeof rest>;
    if (id) {
      await ctx.db.patch(id, updates);
      return id;
    }
    if (existing) {
      await ctx.db.patch(existing._id, updates);
      return existing._id;
    }
    const insertPayload = {
      ...updates,
      ...(args.user_id !== undefined ? { user_id: args.user_id } : {}),
      ...(args.org_id !== undefined ? { org_id: args.org_id } : {}),
    };
    return await ctx.db.insert("users", insertPayload);
  },
});

export const getUserByAuthId = query({
  args: { auth_id: v.string() },
  handler: async (ctx, args) => {
    const user = await ctx.db
      .query("users")
      .withIndex("by_auth_id", (q) => q.eq("auth_id", args.auth_id))
      .unique();
    if (!user) {
      return null;
    }
    return {
      user_id: user.user_id,
      org_id: user.org_id,
      auth_id: user.auth_id ?? null,
      name: user.name ?? null,
      email: user.email ?? null,
      created_at: user.created_at ?? null,
      updated_at: user.updated_at ?? null,
    };
  },
});

export const getUserByEmail = query({
  args: { email: v.string() },
  handler: async (ctx, args) => {
    const email = args.email.trim().toLowerCase();
    const user = await ctx.db
      .query("users")
      .withIndex("email", (q) => q.eq("email", email))
      .unique();
    if (!user) {
      return null;
    }
    return {
      user_id: user.user_id,
      org_id: user.org_id,
      auth_id: user.auth_id ?? null,
      name: user.name ?? null,
      email: user.email ?? null,
      created_at: user.created_at ?? null,
      updated_at: user.updated_at ?? null,
    };
  },
});

export const getUserById = query({
  args: { id: v.id("users") },
  handler: async (ctx, args) => {
    const user = await ctx.db.get(args.id);
    if (!user) {
      return null;
    }
    return {
      user_id: user.user_id ?? null,
      org_id: user.org_id ?? null,
      auth_id: user.auth_id ?? null,
      name: user.name ?? null,
      email: user.email ?? null,
      image: user.image ?? null,
      image_url: user.image_url ?? null,
      created_at: user.created_at ?? null,
      updated_at: user.updated_at ?? null,
    };
  },
});

export const getViewer = query({
  args: {},
  handler: async (ctx) => {
    const userId = await getAuthUserId(ctx);
    if (!userId) {
      return null;
    }
    const user = await ctx.db.get(userId);
    if (!user) {
      return null;
    }
    return {
      user_id: user.user_id ?? null,
      org_id: user.org_id ?? null,
      auth_id: user.auth_id ?? null,
      name: user.name ?? null,
      email: user.email ?? null,
      image_url: user.image_url ?? null,
      created_at: user.created_at ?? null,
      updated_at: user.updated_at ?? null,
    };
  },
});
