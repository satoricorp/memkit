import { Button } from '@/components/ui/button'
import { Copy, Check } from 'lucide-react'
import { siteConfig } from '@/config'

interface PlatformProps {
  apiKey: string
  onCopy: () => void
  copied: boolean
}

export default function Platform({ apiKey, onCopy, copied }: PlatformProps) {
  const baseUrl = process.env.NEXT_PUBLIC_API_BASE_URL ?? "https://api.example.com";
  const model = process.env.NEXT_PUBLIC_API_MODEL ?? "model-name";
  const supportLink =
    siteConfig.footer.links.find((item) => item.label.toLowerCase() === "support")
      ?.href ?? "mailto:support@example.com";
  const setupCommand = `printf '\\nexport PRODUCT_API_BASE_URL="${baseUrl}"\\nexport PRODUCT_API_MODEL="${model}"\\nexport PRODUCT_API_KEY="${apiKey}"\\n' >> ~/.bashrc && source ~/.bashrc`

  return (
    <div className="min-h-screen flex items-center justify-center p-4">
      <div className="w-full max-w-2xl space-y-6">
        <div className="relative flex items-center border-2 border-slate-900 dark:border-white bg-background px-4 py-3 font-mono text-sm rounded-none">
          <span className="pr-10 truncate">{apiKey}</span>
          <Button
            variant="ghost"
            size="icon"
            onClick={onCopy}
            className="absolute right-2 top-1/2 -translate-y-1/2 rounded-none border-0"
          >
            {copied ? <Check className="h-4 w-4" /> : <Copy className="h-4 w-4" />}
          </Button>
        </div>
        <div className="rounded-none border-2 border-slate-900 dark:border-white bg-background p-6 space-y-4">
          <div className="text-lg font-semibold">Instructions to use with your CLI</div>
          <div className="space-y-2 text-sm text-muted-foreground">
            Run this in your terminal to set the API endpoint, model, and auth token.
          </div>
          <pre className="whitespace-pre-wrap break-words rounded-none border border-slate-200 dark:border-slate-700 bg-slate-50 dark:bg-slate-900 px-4 py-3 text-xs sm:text-sm font-mono">
            {setupCommand}
          </pre>
          <div className="text-sm text-muted-foreground">
            Next, run this command in your terminal.
          </div>
          <pre className="whitespace-pre-wrap break-words rounded-none border border-slate-200 dark:border-slate-700 bg-slate-50 dark:bg-slate-900 px-4 py-3 text-xs sm:text-sm font-mono">
            product-cli
          </pre>
          <div className="text-sm text-muted-foreground">
            <p>
              Note: This works for most users on bash. If you use zsh, fish, or nu, update the command for your shell.
            </p>
            <p>
              Need help? Reach out via{" "}
              <a className="underline underline-offset-4" href={supportLink}>
                support
              </a>
              .
            </p>
          </div>
        </div>
      </div>
    </div>
  )
}
