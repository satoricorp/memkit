import { httpAction } from "./_generated/server";
import { httpRouter } from "convex/server";
import { api, internal } from "./_generated/api";
import type { Id } from "./_generated/dataModel";
import { auth } from "./auth";

const http = httpRouter();

auth.addHttpRoutes(http);

const redirectResponse = (location: string) =>
  new Response(null, {
    status: 302,
    headers: {
      Location: location,
    },
  });

function parseCallbackUrl(raw: string) {
  const callback = new URL(raw);
  if (callback.protocol !== "http:") {
    throw new Error("Callback URL must use http");
  }
  if (callback.hostname !== "127.0.0.1" && callback.hostname !== "localhost") {
    throw new Error("Callback host must be 127.0.0.1 or localhost");
  }
  return callback;
}

async function currentUserId(ctx: { auth: { getUserIdentity: () => Promise<any> } }) {
  try {
    const identity = await ctx.auth.getUserIdentity();
    if (!identity?.subject) {
      return null;
    }
    const [userId] = String(identity.subject).split("|");
    return userId ? (userId as Id<"users">) : null;
  } catch {
    return null;
  }
}

function callbackErrorRedirect(callback: URL, state: string | null, message: string) {
  const destination = new URL(callback.toString());
  destination.searchParams.set("error", message);
  if (state) {
    destination.searchParams.set("state", state);
  }
  return destination.toString();
}

async function redirectWithLoginGrant(
  ctx: { runAction: (reference: any, args: Record<string, unknown>) => Promise<{ code: string }> },
  callback: URL,
  state: string,
  userId: Id<"users">,
) {
  const grant = await ctx.runAction((internal as any).cli_auth.issueLoginGrant, {
    user_id: userId,
  });
  const destination = new URL(callback.toString());
  destination.searchParams.set("code", grant.code);
  destination.searchParams.set("state", state);
  return redirectResponse(destination.toString());
}

http.route({
  path: "/api/auth/cli/start",
  method: "GET",
  handler: httpAction(async (ctx, request) => {
    const url = new URL(request.url);
    const rawCallback = url.searchParams.get("callback");
    const state = url.searchParams.get("state");
    if (!rawCallback || !state) {
      return new Response("Missing callback or state", { status: 400 });
    }

    let callback: URL;
    try {
      callback = parseCallbackUrl(rawCallback);
    } catch (error) {
      return new Response(
        error instanceof Error ? error.message : "Invalid callback URL",
        { status: 400 },
      );
    }

    const existingUserId = await currentUserId(ctx);
    if (existingUserId) {
      try {
        return await redirectWithLoginGrant(ctx, callback, state, existingUserId);
      } catch (error) {
        return redirectResponse(
          callbackErrorRedirect(
            callback,
            state,
            error instanceof Error ? error.message : "Unable to create login grant.",
          ),
        );
      }
    }

    const finishPath = `/api/auth/cli/finish?callback=${encodeURIComponent(
      callback.toString(),
    )}&state=${encodeURIComponent(state)}`;
    const signInResult = await ctx.runAction((api as any).auth.signIn, {
      provider: "google",
      params: { redirectTo: finishPath },
      calledBy: "memkit-cli",
    });
    if (!signInResult?.redirect || !signInResult?.verifier) {
      return new Response("Unable to start Google sign-in", { status: 500 });
    }

    // Convex Auth expects the client to keep the verifier and replay it when
    // completing the provider callback. Since the CLI round-trips through
    // browser redirects, we attach it to the finish URL carried in redirectTo.
    const providerRedirect = new URL(signInResult.redirect);
    const rawRedirectTo = providerRedirect.searchParams.get("redirectTo");
    const redirectToUrl = new URL(rawRedirectTo ?? finishPath, request.url);
    redirectToUrl.searchParams.set("verifier", signInResult.verifier);
    providerRedirect.searchParams.set(
      "redirectTo",
      `${redirectToUrl.pathname}${redirectToUrl.search}`,
    );
    return redirectResponse(providerRedirect.toString());
  }),
});

http.route({
  path: "/api/auth/cli/finish",
  method: "GET",
  handler: httpAction(async (ctx, request) => {
    const url = new URL(request.url);
    const rawCallback = url.searchParams.get("callback");
    const state = url.searchParams.get("state");
    const oauthCode = url.searchParams.get("code");
    const verifier = url.searchParams.get("verifier");
    if (!rawCallback || !state) {
      return new Response("Missing callback or state", { status: 400 });
    }

    let callback: URL;
    try {
      callback = parseCallbackUrl(rawCallback);
    } catch (error) {
      return new Response(
        error instanceof Error ? error.message : "Invalid callback URL",
        { status: 400 },
      );
    }

    let userId = await currentUserId(ctx);
    if (!userId && oauthCode && verifier) {
      const session = await ctx.runMutation((internal as any).auth.store, {
        args: {
          type: "verifyCodeAndSignIn",
          params: { code: oauthCode },
          verifier,
          generateTokens: false,
          allowExtraProviders: false,
        },
      });
      userId = session?.userId ?? null;
    }

    if (!userId) {
      return redirectResponse(
        callbackErrorRedirect(callback, state, "Google sign-in did not complete."),
      );
    }

    try {
      return await redirectWithLoginGrant(ctx, callback, state, userId);
    } catch (error) {
      return redirectResponse(
        callbackErrorRedirect(
          callback,
          state,
          error instanceof Error ? error.message : "Unable to create login grant.",
        ),
      );
    }
  }),
});

export default http;
