"use node";

import { action } from "./_generated/server";
import { getAuthUserId } from "@convex-dev/auth/server";
import { createHash, randomBytes } from "crypto";
import { api } from "./_generated/api";

type OrgRecord = {
  org_id: number;
  api_key?: string | null;
  name?: string | null;
  email?: string | null;
  created_at?: string | null;
  updated_at?: string | null;
};

type UserRecord = {
  user_id?: number | null;
  org_id?: number | null;
  auth_id?: string | null;
  email?: string | null;
  name?: string | null;
  image?: string | null;
  image_url?: string | null;
};

const stableId = (value: string) => {
  const digest = createHash("sha256").update(value).digest();
  const hex = digest.subarray(0, 8).toString("hex");
  return Number(BigInt.asUintN(53, BigInt(`0x${hex}`)));
};

const generateApiKey = () => {
  const prefix = process.env.API_KEY_PREFIX ?? "app";
  return `${prefix}-${randomBytes(24).toString("base64url")}`;
};

const hashApiKey = (apiKey: string, salt: string) =>
  createHash("sha256").update(`${apiKey}${salt}`).digest("hex");

export const ensureUserAccount = action({
  args: {},
  handler: async (ctx): Promise<{ api_key: string }> => {
    const identity = await ctx.auth.getUserIdentity();
    if (!identity) {
      throw new Error("Unauthorized");
    }
    const authUserId = await getAuthUserId(ctx);
    if (!authUserId) {
      throw new Error("Unauthorized");
    }
    const salt = process.env.API_KEY_SALT ?? "";
    if (!salt) {
      throw new Error("API_KEY_SALT is not configured");
    }

    const authUser = (await ctx.runQuery(api.users.getUserById, {
      id: authUserId,
    })) as UserRecord | null;
    if (!authUser?.email) {
      throw new Error("Missing email from OAuth");
    }

    const authUserIdString = String(authUserId);
    const userId = authUser.user_id ?? stableId(`${authUserIdString}:user`);

    if (authUser.org_id) {
      const org = (await ctx.runQuery(api.orgs.getOrgById, {
        org_id: authUser.org_id,
      })) as OrgRecord | null;
      if (org?.api_key) {
        return { api_key: org.api_key };
      }
    }

    const now = new Date().toISOString();
    const email = authUser.email;
    const orgId = (await ctx.runQuery(api.orgs.getOrgIdByEmail, { email })) as number | null;
    if (orgId) {
      const org = (await ctx.runQuery(api.orgs.getOrgById, { org_id: orgId })) as OrgRecord | null;
      const apiKey: string = org?.api_key ?? generateApiKey();
      const apiKeyHash = hashApiKey(apiKey, salt);
      await ctx.runMutation(api.orgs.upsertOrg, {
        org_id: orgId,
        api_key: apiKey,
        api_key_hash: apiKeyHash,
        name: org?.name ?? identity.name ?? undefined,
        email,
        created_at: org?.created_at ?? now,
        updated_at: now,
      });
      await ctx.runMutation(api.users.upsertUser, {
        id: authUserId,
        user_id: userId,
        org_id: orgId,
        auth_id: authUserIdString,
        name: authUser.name ?? undefined,
        email,
        image_url: authUser.image_url ?? authUser.image ?? undefined,
        admin: true,
        created_at: now,
        updated_at: now,
      });
      return { api_key: apiKey };
    }

    const newOrgId = stableId(`${authUserIdString}:org`);
    const apiKey = generateApiKey();
    const apiKeyHash = hashApiKey(apiKey, salt);

    await ctx.runMutation(api.orgs.upsertOrg, {
      org_id: newOrgId,
      api_key: apiKey,
      api_key_hash: apiKeyHash,
      name: identity.name ?? undefined,
      email,
      created_at: now,
    });

    await ctx.runMutation(api.users.upsertUser, {
      id: authUserId,
      user_id: userId,
      org_id: newOrgId,
      auth_id: authUserIdString,
      name: authUser.name ?? undefined,
      email,
      image_url: authUser.image_url ?? authUser.image ?? undefined,
      admin: true,
      created_at: now,
      updated_at: now,
    });

    return { api_key: apiKey };
  },
});
