"use client";

import { useEffect, useState } from "react";
import { Github, MessageCircle } from "lucide-react";
import ThemeToggle from "@/components/theme-toggle";
import { siteConfig } from "@/config";

function XLogoIcon() {
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true" className="h-4 w-4 fill-current">
      <path d="M18.244 2h3.308l-7.227 8.26L23 22h-6.406l-5.016-6.57L5.83 22H2.52l7.73-8.835L1 2h6.568l4.534 5.996L18.244 2Zm-1.161 18h1.833L6.574 3.875H4.607L17.083 20Z" />
    </svg>
  );
}

function LinkedInLogoIcon() {
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true" className="h-4 w-4 fill-current">
      <path d="M20.45 20.45h-3.554v-5.569c0-1.328-.027-3.036-1.851-3.036-1.853 0-2.136 1.447-2.136 2.94v5.665H9.355V9h3.413v1.561h.049c.476-.9 1.637-1.85 3.369-1.85 3.602 0 4.267 2.37 4.267 5.455v6.284ZM5.347 7.434a2.063 2.063 0 1 1 0-4.127 2.063 2.063 0 0 1 0 4.127ZM7.124 20.45H3.566V9h3.558v11.45ZM22.225 0H1.771C.792 0 0 .774 0 1.729v20.542C0 23.227.792 24 1.771 24h20.451C23.2 24 24 23.227 24 22.271V1.729C24 .774 23.2 0 22.222 0h.003Z" />
    </svg>
  );
}

export default function Footer() {
  const [mounted, setMounted] = useState(false);

  const socialIcons: Record<string, JSX.Element> = {
    x: <XLogoIcon />,
    github: <Github className="h-4 w-4" />,
    discord: <MessageCircle className="h-4 w-4" />,
    linkedin: <LinkedInLogoIcon />,
  };

  useEffect(() => {
    setMounted(true);
  }, []);

  if (!mounted) {
    return null;
  }

  return (
    <footer className="border-t border-border bg-background">
      <div className="mx-auto grid w-full max-w-6xl gap-8 border-x border-border px-6 py-12 md:grid-cols-[1.6fr_1fr]">
        <div className="space-y-4">
          <div className="text-2xl font-brand">{siteConfig.footer.headline}</div>
          <p className="text-sm text-muted-foreground max-w-md">
            {siteConfig.footer.subhead}
          </p>
        </div>
        <div className="grid justify-items-end gap-4 text-right text-sm">
          <div className="flex flex-col items-end gap-2">
            {siteConfig.footer.links.map((item) => (
              <a
                key={item.label}
                href={item.href}
                className="text-muted-foreground transition-colors hover:text-foreground"
              >
                {item.label}
              </a>
            ))}
          </div>
          <div className="flex flex-wrap justify-end gap-3 text-xs text-muted-foreground">
            {siteConfig.social.map((item) => (
              <a
                key={item.label}
                href={item.href}
                aria-label={item.label}
                className="hover:text-foreground"
              >
                {socialIcons[item.label.toLowerCase()] ?? item.label}
              </a>
            ))}
          </div>
        </div>
      </div>
      <div className="mx-auto flex w-full max-w-6xl items-center justify-between border-x border-t border-border px-6 py-4">
        <ThemeToggle />
        <div className="text-xs text-muted-foreground">{siteConfig.footer.legal}</div>
      </div>
    </footer>
  );
}
