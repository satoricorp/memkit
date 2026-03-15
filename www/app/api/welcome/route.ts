'use server'

import { sendWelcomeEmail } from '@/app/actions/resend'

export async function POST(request: Request) {
  try {
    const { email, orgName } = await request.json()

    if (!email || !orgName) {
      return Response.json({ error: 'Email and orgName are required' }, { status: 400 })
    }

    const result = await sendWelcomeEmail(email, orgName)

    if (!result.success) {
      return Response.json({ error: result.error }, { status: 500 })
    }

    return Response.json({ success: true })
  } catch (error) {
    console.error('Error in send-welcome-email API:', error)
    return Response.json({ error: 'Failed to send email' }, { status: 500 })
  }
}