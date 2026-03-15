"use client";
import { useState, useEffect, useRef } from 'react'
import { createUserAccount } from '@/app/actions/users'
import { useRouter } from 'next/navigation'
import { useQuery } from 'convex/react'
import { useAuthToken } from '@convex-dev/auth/react'
import { api } from '@/convex/_generated/api'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import Platform from '@/components/platform'

export default function PlatformWrapper() {
  const [apiKey, setApiKey] = useState('')
  const [creatingAccount, setCreatingAccount] = useState(false)
  const [accountError, setAccountError] = useState('')
  const [copied, setCopied] = useState(false)
  const requestRef = useRef(0)
  const router = useRouter()
  const token = useAuthToken()
  const isAuthenticated = Boolean(token)
  const viewer = useQuery(api.users.getViewer)

  useEffect(() => {
    if (!isAuthenticated || apiKey || creatingAccount || accountError) return
    createAccount()
  }, [isAuthenticated, apiKey, creatingAccount, accountError])

  const sendWelcomeEmail = (email: string | undefined) => {
    if (!email) return
    
    fetch('/api/welcome', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ email }),
    }).catch((err) => console.error('Failed to send welcome email:', err))
  }

  const withTimeout = async <T,>(promise: Promise<T>, ms: number) => {
    let timeoutId: ReturnType<typeof setTimeout> | undefined
    try {
      return await Promise.race([
        promise,
        new Promise<T>((_, reject) => {
          timeoutId = setTimeout(() => reject(new Error('Account setup timed out')), ms)
        }),
      ])
    } finally {
      if (timeoutId) clearTimeout(timeoutId)
    }
  }

  const createAccount = async () => {
    const requestId = requestRef.current + 1
    requestRef.current = requestId
    try {
      setCreatingAccount(true)
      setAccountError('')

      const data = await withTimeout(createUserAccount(), 12000)
      if (requestRef.current !== requestId) return
      if (!data?.api_key) {
        throw new Error('API key missing from response')
      }
      setApiKey(data.api_key)
      sendWelcomeEmail(viewer?.email ?? undefined)
    } catch (err) {
      if (requestRef.current !== requestId) return
      setAccountError(err instanceof Error ? err.message : 'An error occurred')
    } finally {
      if (requestRef.current === requestId) {
        setCreatingAccount(false)
      }
    }
  }

  const retryAccountSetup = () => {
    setAccountError('')
    createAccount()
  }

  const copyToClipboard = async () => {
    try {
      await navigator.clipboard.writeText(apiKey)
      setCopied(true)
      setTimeout(() => setCopied(false), 2000)
    } catch (err) {
      console.error('Failed to copy:', err)
    }
  }

  if (!isAuthenticated) {
    return null
  }

  if (creatingAccount) {
    return (
      <div className="min-h-screen flex items-center justify-center">
        <div className="text-center">
          <div className="animate-spin rounded-full h-8 w-8 border-b-2 border-black mx-auto mb-4"></div>
          <p>Setting up your account...</p>
        </div>
      </div>
    )
  }

  if (accountError) {
    return (
      <div className="min-h-screen flex items-center justify-center p-4">
        <Card className="w-full max-w-md">
          <CardHeader>
            <CardTitle>Something went wrong</CardTitle>
            <CardDescription>{accountError}</CardDescription>
          </CardHeader>
          <CardContent>
            <div className="flex flex-col gap-2">
              <Button onClick={retryAccountSetup} className="w-full">
                Try again
              </Button>
              <Button variant="outline" onClick={() => router.push('/')} className="w-full">
                Go Home
              </Button>
            </div>
          </CardContent>
        </Card>
      </div>
    )
  }

  if (apiKey) {
    return <Platform apiKey={apiKey} onCopy={copyToClipboard} copied={copied} />
  }

  return (
    <div className="min-h-screen flex items-center justify-center">
      <div className="text-center">
        <div className="animate-spin rounded-full h-8 w-8 border-b-2 border-black mx-auto mb-4"></div>
        <p>Loading your account...</p>
      </div>
    </div>
  )
}
