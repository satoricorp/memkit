'use server'

import { convexAuthNextjsToken } from '@convex-dev/auth/nextjs/server'
import { api } from '@/convex/_generated/api'
import { UserCreationResponse } from '@/types/platform'
import { fetchAction } from 'convex/nextjs'

export async function createUserAccount(): Promise<UserCreationResponse> {
  const token = await convexAuthNextjsToken()
  if (!token) {
    throw new Error('Unauthorized')
  }
  return await fetchAction(api.users_actions.ensureUserAccount, {}, { token })
}

export async function createUserIfMissing(): Promise<void> {
  await createUserAccount()
}
