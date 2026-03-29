'use server'

import { Polar } from '@polar-sh/sdk'
import { convexAuthNextjsToken } from '@convex-dev/auth/nextjs/server'
import { fetchQuery } from 'convex/nextjs'
import { api } from '@convex-generated/api'

const polar = new Polar({
  serverURL: process.env.NODE_ENV === 'development'
    ? "https://sandbox-api.polar.sh"
    : "https://api.polar.sh",
  accessToken: process.env.NODE_ENV === 'development'
    ? process.env.POLAR_SANDBOX_ACCESS_TOKEN ?? ''
    : process.env.POLAR_ACCESS_TOKEN ?? '',
})

export async function hasPlan() {
  try {
    const token = await convexAuthNextjsToken()
    if (!token) {
      return { success: false, error: 'Unauthorized' }
    }
    const viewer = await fetchQuery(api.users.getViewer, {}, { token })
    if (!viewer?.org_id) {
      return { success: false, error: 'Organization not found', hasActiveSubscription: false }
    }
    const state = await polar.customers.getStateExternal({ externalId: String(viewer.org_id) });
    const hasActiveSubscription = state.activeSubscriptions.length > 0;

    return { success: true, hasActiveSubscription }
  } catch (error) {
    return { success: false, error: 'Failed to check plan', hasActiveSubscription: false }
  }
}

export async function getCustomerPortalUrl() {
  try {
    const token = await convexAuthNextjsToken()
    if (!token) {
      return { success: false, error: 'Unauthorized' }
    }
    const viewer = await fetchQuery(api.users.getViewer, {}, { token })
    if (!viewer?.org_id) {
      return { success: false, error: 'Organization not found' }
    }
    const state = await polar.customers.getStateExternal({ externalId: String(viewer.org_id) });
    
    if (state.activeSubscriptions.length === 0) {
      return { success: false, error: 'No active subscription' }
    }

    // Use Polar's customerSessions.create to generate a pre-authenticated portal URL
    if (!state.id) {
      return { success: false, error: 'Customer ID not found' }
    }

    const session = await polar.customerSessions.create({
      customerId: state.id,
    })

    return { success: true, portalUrl: session.customerPortalUrl }
  } catch (error) {
    console.error('Error getting customer portal URL:', error)
    return { success: false, error: 'Failed to get customer portal URL' }
  }
}
