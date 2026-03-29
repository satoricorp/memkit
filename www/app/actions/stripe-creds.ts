"use server";

import { convexAuthNextjsToken } from '@convex-dev/auth/nextjs/server'
import { fetchQuery } from 'convex/nextjs'
import { api } from '@convex-generated/api'

export type PolarCredsStatus = {
  hasPolarCreds: boolean
}

export async function getPolarCredsStatus(): Promise<PolarCredsStatus> {
  const token = await convexAuthNextjsToken()
  if (!token) {
    return { hasPolarCreds: false }
  }
  const result = await fetchQuery(api.orgs.getPolarCredsForViewer, {}, { token })
  return { hasPolarCreds: Boolean(result?.hasPolarCreds) }
}
