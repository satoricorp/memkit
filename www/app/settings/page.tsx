"use client";
import { useState } from 'react'
import { getCustomerPortalUrl } from '@/app/actions/polar'
import { Button } from '@/components/ui/button'
import { ExternalLink } from 'lucide-react'
import Nav from '@/components/nav'
import Footer from '@/components/footer'

export default function SettingsPage() {
  const [isLoadingPortal, setIsLoadingPortal] = useState(false)

  const handleOpenPortal = async () => {
    setIsLoadingPortal(true)
    try {
      const result = await getCustomerPortalUrl()
      if (result.success && result.portalUrl) {
        window.open(result.portalUrl, '_blank', 'noopener,noreferrer')
      } else {
      }
    } catch (error) {
    } finally {
      setIsLoadingPortal(false)
    }
  }

  return (
    <>
      <Nav />
      <main className="flex min-h-screen flex-col items-center justify-center pt-20 pb-20 px-4">
        <div className="w-full max-w-2xl space-y-6">
          <div className="space-y-2">
            <h1 className="text-3xl font-bold">Settings</h1>
            <p className="text-muted-foreground">Manage your account and subscription</p>
            <Button
              onClick={handleOpenPortal}
              disabled={isLoadingPortal}
              className="flex items-center gap-2"
            >
              {isLoadingPortal ? 'Loading...' : 'Open Customer Portal'}
              <ExternalLink className="w-4 h-4" />
            </Button>
          </div>
        </div>
      </main>
      <Footer />
    </>
  )
}
