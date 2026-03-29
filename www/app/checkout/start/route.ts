import { NextRequest, NextResponse } from 'next/server'
import { convexAuthNextjsToken, isAuthenticatedNextjs } from '@convex-dev/auth/nextjs/server'
import { api } from '@convex-generated/api'
import { Polar } from '@polar-sh/sdk'
import { fetchAction, fetchQuery } from 'convex/nextjs'
const POLAR_PRODUCT_ID = process.env.POLAR_PRODUCT_ID ?? ''
const POLAR_SANDBOX_PRODUCT_ID = process.env.POLAR_SANDBOX_PRODUCT_ID ?? ''

const getPolarClient = () =>
  new Polar({
    serverURL: process.env.NODE_ENV === 'development'
      ? 'https://sandbox-api.polar.sh'
      : 'https://api.polar.sh',
    accessToken: process.env.NODE_ENV === 'development'
      ? process.env.POLAR_SANDBOX_ACCESS_TOKEN ?? ''
      : process.env.POLAR_ACCESS_TOKEN ?? '',
  })

const getSuccessUrl = () =>
  process.env.SUCCESS_URL ??
  `${process.env.NEXT_PUBLIC_APP_URL ?? 'http://localhost:3000'}?checkout=success`

const getReturnUrl = () =>
  process.env.NEXT_PUBLIC_APP_URL ?? 'http://localhost:3000'

export async function GET(request: NextRequest) {
  const debug = request.nextUrl.searchParams.get('debug') === '1'
  const authed = await isAuthenticatedNextjs()
  if (!authed) {
    return NextResponse.redirect(new URL('/signin', request.url))
  }
  const token = await convexAuthNextjsToken()
  if (!token) {
    return NextResponse.redirect(new URL('/signin', request.url))
  }

  let createError: string | null = null
  try {
    await fetchAction(api.users_actions.ensureUserAccount, {}, { token })
  } catch (err) {
    createError = err instanceof Error ? err.message : 'Failed to create user before checkout'
    console.error('Failed to create user before checkout', err)
  }

  const viewer = await fetchQuery(api.users.getViewer, {}, { token })
  if (!viewer?.org_id) {
    if (debug) {
      return NextResponse.json({
        ok: false,
        reason: 'missing_org',
        createError,
        org: viewer,
      })
    }
    return NextResponse.redirect(new URL('/', request.url))
  }
  if (!viewer.email) {
    if (debug) {
      return NextResponse.json({
        ok: false,
        reason: 'missing_email',
        createError,
        org: viewer,
      })
    }
    return NextResponse.redirect(new URL('/signin', request.url))
  }
  const polar = getPolarClient()
  const productId =
    process.env.NODE_ENV === 'development' ? POLAR_SANDBOX_PRODUCT_ID : POLAR_PRODUCT_ID
  if (!productId) {
    if (debug) {
      return NextResponse.json({
        ok: false,
        reason: 'missing_product_id',
        createError,
      })
    }
    return NextResponse.redirect(new URL('/settings', request.url))
  }
  try {
    const state = await polar.customers.getStateExternal({ externalId: String(viewer.org_id) })
    const hasActiveSubscription = state.activeSubscriptions.length > 0
    if (hasActiveSubscription) {
      if (debug) {
        return NextResponse.json({
          ok: false,
          reason: 'has_active_subscription',
          createError,
          state,
        })
      }
      return NextResponse.redirect(new URL('/', request.url))
    }
  } catch (err) {
    if (debug) {
      return NextResponse.json({
        ok: false,
        reason: 'polar_state_error',
        createError,
        error: err instanceof Error ? err.message : String(err),
      })
    }
  }
  const checkout = await polar.checkouts.create({
    products: [productId],
    externalCustomerId: String(viewer.org_id),
    customerEmail: viewer.email,
    metadata: { org_id: String(viewer.org_id) },
    successUrl: getSuccessUrl(),
    returnUrl: getReturnUrl(),
  })
  if (!checkout?.url) {
    if (debug) {
      return NextResponse.json({
        ok: false,
        reason: 'missing_checkout_url',
        createError,
      })
    }
    return NextResponse.redirect(new URL('/', request.url))
  }
  return NextResponse.redirect(checkout.url)
}
