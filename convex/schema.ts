import { defineSchema, defineTable } from "convex/server";
import { v } from "convex/values";
import { authTables } from "@convex-dev/auth/server";

export default defineSchema({
  ...authTables,
  users: defineTable({
    name: v.optional(v.string()),
    image: v.optional(v.string()),
    email: v.optional(v.string()),
    emailVerificationTime: v.optional(v.number()),
    phone: v.optional(v.string()),
    phoneVerificationTime: v.optional(v.number()),
    isAnonymous: v.optional(v.boolean()),
    user_id: v.optional(v.number()),
    org_id: v.optional(v.number()),
    auth_id: v.optional(v.string()),
    admin: v.optional(v.boolean()),
    first_name: v.optional(v.string()),
    last_name: v.optional(v.string()),
    username: v.optional(v.string()),
    image_url: v.optional(v.string()),
    created_at: v.optional(v.string()),
    updated_at: v.optional(v.string()),
  })
    .index("by_user_id", ["user_id"])
    .index("by_auth_id", ["auth_id"])
    .index("email", ["email"]),
  orgs: defineTable({
    org_id: v.number(),
    name: v.optional(v.string()),
    email: v.optional(v.string()),
    api_key: v.optional(v.string()),
    api_key_hash: v.string(),
    polar_customer_id: v.optional(v.union(v.string(), v.null())),
    polar_subscription_id: v.optional(v.union(v.string(), v.null())),
    stripe_customer_id: v.optional(v.string()),
    stripe_subscription_id: v.optional(v.string()),
    stripe_subscription_item_id: v.optional(v.string()),
    created_at: v.optional(v.string()),
    updated_at: v.optional(v.string()),
  })
    .index("by_org_id", ["org_id"])
    .index("by_api_key_hash", ["api_key_hash"]),
  cliLoginGrants: defineTable({
    code_hash: v.string(),
    user_id: v.id("users"),
    expiration_time: v.number(),
    used_time: v.optional(v.number()),
    created_at: v.string(),
  }).index("by_code_hash", ["code_hash"]),
  cliSessions: defineTable({
    token_hash: v.string(),
    user_id: v.id("users"),
    expiration_time: v.number(),
    last_used_time: v.number(),
    revoked_time: v.optional(v.number()),
    created_at: v.string(),
    updated_at: v.string(),
  })
    .index("by_token_hash", ["token_hash"])
    .index("by_user_id", ["user_id"]),
});
