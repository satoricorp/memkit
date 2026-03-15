"use client";

import Link from "next/link";
import { useState } from "react";
import { Command, Menu, X } from "lucide-react";
import { useAuthActions, useAuthToken } from "@convex-dev/auth/react";
import { siteConfig } from "@/config";
import Logo from "@/components/logo";
import { Button } from "@/components/ui/button";
import CommandPalette from "@/components/command-palette";

export default function Nav() {
  const token = useAuthToken();
  const isAuthenticated = Boolean(token);
  const { signOut } = useAuthActions();
  const [open, setOpen] = useState(false);
  const [commandOpen, setCommandOpen] = useState(false);

  const navItems = siteConfig.nav.items;

  return (
    <>
      <nav className="border-b border-border/70 bg-background/90 backdrop-blur">
        <div className="mx-auto w-full max-w-6xl border-x border-border frame-corners-bottom">
          <div className="flex items-stretch justify-between px-4">
            <div className="flex items-center py-4">
              <Logo />
            </div>
            <div className="ml-auto hidden items-stretch gap-3 md:flex">
              <div className="flex items-center gap-8 px-6 text-sm">
                {navItems.map((item) => (
                  <a
                    key={item.label}
                    href={item.href}
                    className="text-muted-foreground transition-colors hover:text-foreground"
                  >
                    {item.label}
                  </a>
                ))}
              </div>
              <div className="flex items-center gap-10 px-4 py-4">
                {isAuthenticated ? (
                  <>
                    <button
                      onClick={() => setCommandOpen(true)}
                      className="flex items-center gap-2 rounded-full border border-border px-3 py-1.5 text-xs text-muted-foreground transition-colors hover:text-foreground"
                    >
                      <Command className="size-3" />
                      Command
                    </button>
                    <Link
                      href="/dashboard"
                      className="text-sm text-muted-foreground transition-colors hover:text-foreground"
                    >
                      Dashboard
                    </Link>
                    <Button variant="outline" onClick={() => void signOut()}>
                      Sign out
                    </Button>
                  </>
                ) : (
                  <>
                    <Button asChild className="px-6">
                      <Link href="/signin">{siteConfig.nav.cta.label}</Link>
                    </Button>
                  </>
                )}
              </div>
            </div>
            <button
              className="md:hidden my-4 flex items-center justify-center size-9"
              onClick={() => setOpen((prev) => !prev)}
              aria-label="Toggle menu"
            >
              {open ? <X className="size-4" /> : <Menu className="size-4" />}
            </button>
          </div>
        </div>
      </nav>
      {open && (
        <div className="border-b border-border bg-background md:hidden">
          <div className="mx-auto flex w-full max-w-6xl flex-col gap-4 border-x border-border px-6 py-6">
            <div className="flex flex-col gap-3">
              {navItems.map((item) => (
                <a
                  key={item.label}
                  href={item.href}
                  className="text-sm text-muted-foreground transition-colors hover:text-foreground"
                  onClick={() => setOpen(false)}
                >
                  {item.label}
                </a>
              ))}
            </div>
            <div className="flex flex-col gap-3 border-t border-border pt-4">
              {isAuthenticated ? (
                <>
                  <button
                    onClick={() => setCommandOpen(true)}
                    className="flex items-center gap-2 rounded-full border border-border px-3 py-1.5 text-xs text-muted-foreground transition-colors hover:text-foreground"
                  >
                    <Command className="size-3" />
                    Command
                  </button>
                  <Link
                    href="/dashboard"
                    className="text-sm text-muted-foreground transition-colors hover:text-foreground"
                    onClick={() => setOpen(false)}
                  >
                    Dashboard
                  </Link>
                  <Button variant="outline" onClick={() => void signOut()}>
                    Sign out
                  </Button>
                </>
              ) : (
                <>
                  <Button asChild className="px-6">
                    <Link href="/signin" onClick={() => setOpen(false)}>
                      {siteConfig.nav.cta.label}
                    </Link>
                  </Button>
                </>
              )}
            </div>
          </div>
        </div>
      )}
      <CommandPalette open={commandOpen} onOpenChange={setCommandOpen} isSignedIn={isAuthenticated} />
    </>
  );
}
