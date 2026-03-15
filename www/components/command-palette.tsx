"use client"

import * as React from "react"
import {
  CommandDialog,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
  CommandSeparator,
  CommandShortcut,
} from "@/components/ui/command";
import { useRouter } from "next/navigation";
import { siteConfig } from "@/config";

interface CommandPaletteProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  isSignedIn: boolean;
}

export default function CommandPalette({ open, onOpenChange, isSignedIn }: CommandPaletteProps) {
  const router = useRouter();
  const gPressedRef = React.useRef(false);
  const timeoutRef = React.useRef<NodeJS.Timeout | null>(null);
  
  React.useEffect(() => {
    // Only enable keyboard shortcut when signed in
    if (!isSignedIn) return;
    
    const down = (e: KeyboardEvent) => {
      // Don't trigger shortcuts when typing in input fields
      if (e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement) {
        return;
      }

      // Cmd/Ctrl + K to open command palette
      if (e.key === "k" && (e.metaKey || e.ctrlKey)) {
        e.preventDefault()
        onOpenChange(!open)
        return
      }

      // G + S sequence for settings
      if (e.key === "g" || e.key === "G") {
        // Reset any existing timeout
        if (timeoutRef.current) {
          clearTimeout(timeoutRef.current)
        }
        gPressedRef.current = true
        
        // Reset after 1 second if S isn't pressed
        timeoutRef.current = setTimeout(() => {
          gPressedRef.current = false
        }, 1000)
        return
      }

      if ((e.key === "s" || e.key === "S") && gPressedRef.current) {
        e.preventDefault()
        gPressedRef.current = false
        if (timeoutRef.current) {
          clearTimeout(timeoutRef.current)
          timeoutRef.current = null
        }
        router.push('/settings')
      }
    }
    
    document.addEventListener("keydown", down)
    return () => {
      document.removeEventListener("keydown", down)
      if (timeoutRef.current) {
        clearTimeout(timeoutRef.current)
      }
    }
  }, [open, onOpenChange, isSignedIn, router])

  return (
    <CommandDialog open={open} onOpenChange={onOpenChange} className="border-0">
      <CommandInput placeholder="Type a command or search..." />
      <CommandList>
        <CommandEmpty>No results found.</CommandEmpty>
        {isSignedIn ? (
          <>
            <CommandGroup heading="Actions">
              <CommandItem
                onSelect={() => {
                  onOpenChange(false);
                  router.push("/dashboard");
                }}
              >
                <span>Go to Dashboard</span>
              </CommandItem>
              <CommandItem
                onSelect={() => {
                  onOpenChange(false);
                  router.push("/settings");
                }}
              >
                <span>Go to Settings</span>
                <CommandShortcut>G + S</CommandShortcut>
              </CommandItem>
            </CommandGroup>
            <CommandSeparator />
            <CommandGroup heading="Marketing">
              {siteConfig.nav.items.map((item) => (
                <CommandItem
                  key={item.label}
                  onSelect={() => {
                    onOpenChange(false);
                    const destination = item.href.startsWith("#")
                      ? `/${item.href}`
                      : item.href;
                    router.push(destination);
                  }}
                >
                  <span>{item.label}</span>
                </CommandItem>
              ))}
            </CommandGroup>
          </>
        ) : (
          <>
            <CommandGroup heading="Get Started">
              <CommandItem
                onSelect={() => {
                  onOpenChange(false);
                  router.push("/signin");
                }}
              >
                <span>Sign in to continue</span>
              </CommandItem>
              <CommandItem
                onSelect={() => {
                  onOpenChange(false);
                  router.push("/");
                }}
              >
                <span>Return to home</span>
              </CommandItem>
            </CommandGroup>
          </>
        )}
      </CommandList>
    </CommandDialog>
  );
}
