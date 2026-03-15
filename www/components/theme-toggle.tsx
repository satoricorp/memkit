"use client";

import { useTheme } from "next-themes";
import { Moon, Sun } from "lucide-react";
import { Switch } from "@/components/ui/switch";

export default function ThemeToggle() {
  const { theme, setTheme, resolvedTheme } = useTheme();
  const currentTheme = resolvedTheme || theme;
  const isDark = currentTheme === "dark";

  return (
    <div className="flex items-center gap-1.5 bg-background/80">
      <Sun className="h-3 w-3 text-foreground/70" aria-hidden="true" />
      <Switch
        checked={isDark}
        onCheckedChange={(checked) => setTheme(checked ? "dark" : "light")}
        aria-label="Toggle theme"
        className="h-3 w-5 border-border/90 data-[state=unchecked]:bg-muted [&_[data-slot=switch-thumb]]:size-2.5 [&_[data-slot=switch-thumb]]:data-[state=checked]:translate-x-[calc(100%-2px)]"
      />
      <Moon className="h-3 w-3 text-foreground/70" aria-hidden="true" />
    </div>
  );
}

