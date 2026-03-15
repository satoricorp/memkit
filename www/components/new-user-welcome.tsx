import { useMemo, useState } from 'react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { Copy, Check } from 'lucide-react'
import { siteConfig } from '@/config'

interface ApiKeyWelcomeProps {
  apiKey: string
  onViewChange: (view: 'api-key' | 'dashboard') => void
}

export default function NewUserWelcome({ apiKey, onViewChange }: ApiKeyWelcomeProps) {
  const [copied, setCopied] = useState(false)
  const docsLink = useMemo(() => {
    const match = siteConfig.footer.links.find(
      (item) => item.label.toLowerCase() === 'docs',
    )
    return match ?? siteConfig.footer.links[0]
  }, [])

  const copyToClipboard = async () => {
    try {
      await navigator.clipboard.writeText(apiKey)
      setCopied(true)
      setTimeout(() => setCopied(false), 2000)
    } catch (err) {
      console.error('Failed to copy:', err)
    }
  }

  return (
    <div className="min-h-screen flex items-center justify-center p-4">
        <Card className="w-full max-w-md">
          <CardHeader>
          <CardTitle>Welcome to {siteConfig.brand.name}!</CardTitle>
          <CardDescription>
            Your account has been created successfully. Here's your API key:
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-4">
          <div className="space-y-2">
            <Label>API Key</Label>
            <div className="flex gap-2">
              <Input
                value={apiKey}
                readOnly
                className="font-mono text-sm"
              />
              <Button
                variant="outline"
                size="icon"
                onClick={copyToClipboard}
              >
                {copied ? <Check className="h-4 w-4" /> : <Copy className="h-4 w-4" />}
              </Button>
            </div>
          </div>
          <p className="text-sm text-muted-foreground">
            Keep this key secure. You won't be able to see it again.
          </p>
          <div className="flex flex-col sm:flex-row gap-2">
            <Button
              onClick={() => onViewChange('dashboard')}
              className="flex-1"
            >
              Go to Dashboard
            </Button>
            <Button
              variant="outline"
              onClick={() => window.open(docsLink?.href ?? '/', '_blank')}
              className="flex-1"
            >
              View Docs
            </Button>
          </div>
        </CardContent>
      </Card>
    </div>
  )
}
