"use client";

import { useEffect, useState } from "react";
import styles from "./page.module.css";

type Theme = "light" | "dark";

const installCommands = [
  {
    label: "Installation",
    command: "npx expect-cli@latest init",
  },
  {
    label: "Add skill",
    command: "npx skills add https://github.com/millionco/expect --skill expect",
  },
];

const links = [
  {
    label: "GitHub",
    href: "https://github.com/millionco/expect",
  },
  {
    label: "X",
    href: "https://x.com/aidenybai",
  },
];

function CopyButton({
  command,
  copied,
  onCopy,
}: {
  command: string;
  copied: boolean;
  onCopy: (value: string) => void;
}) {
  return (
    <button
      type="button"
      className={styles.copyButton}
      onClick={() => onCopy(command)}
      aria-label={`Copy ${command}`}
    >
      {copied ? (
        <svg viewBox="0 0 24 24" aria-hidden="true">
          <path
            d="M5 13.2 9.2 17.4 19 7.6"
            fill="none"
            stroke="currentColor"
            strokeLinecap="round"
            strokeLinejoin="round"
            strokeWidth="1.7"
          />
        </svg>
      ) : (
        <svg viewBox="0 0 24 24" aria-hidden="true">
          <path
            d="M9 15c0-2.828 0-4.243.879-5.121S12.172 9 15 9h1c2.828 0 4.243 0 5.121.879S22 12.172 22 15v1c0 2.828 0 4.243-.879 5.121S18.828 22 16 22h-1c-2.828 0-4.243 0-5.121-.879S9 18.828 9 16z"
            fill="none"
            stroke="currentColor"
            strokeLinecap="round"
            strokeLinejoin="round"
            strokeWidth="1.5"
          />
          <path
            d="M17 9c-.003-2.957-.047-4.489-.908-5.538a4.76 4.76 0 0 0-.554-.554C14.431 2 12.787 2 9.5 2S4.569 2 3.462 2.908a4.76 4.76 0 0 0-.554.554C2 4.569 2 6.213 2 9.5s0 4.931.908 6.038c.166.202.352.388.554.554C4.511 16.953 6.043 16.997 9 17"
            fill="none"
            stroke="currentColor"
            strokeLinecap="round"
            strokeLinejoin="round"
            strokeWidth="1.5"
          />
        </svg>
      )}
    </button>
  );
}

function ThemeToggle({
  theme,
  setTheme,
}: {
  theme: Theme;
  setTheme: (value: Theme) => void;
}) {
  return (
    <div className={styles.themeToggle} aria-label="Theme toggle">
      <button
        type="button"
        className={theme === "light" ? styles.themeButtonActive : styles.themeButton}
        onClick={() => setTheme("light")}
        aria-label="Switch to light theme"
      >
        <svg viewBox="0 0 24 24" aria-hidden="true">
          <circle cx="12" cy="12" r="4.4" fill="none" stroke="currentColor" strokeWidth="1.5" />
          <path
            d="M12 2.5V4.4M12 19.6v1.9M19.5 12h1.9M2.6 12H4.5M18.7 5.3l-1.4 1.4M6.7 17.3l-1.4 1.4M18.7 18.7l-1.4-1.4M6.7 6.7 5.3 5.3"
            fill="none"
            stroke="currentColor"
            strokeLinecap="round"
            strokeWidth="1.5"
          />
        </svg>
      </button>
      <button
        type="button"
        className={theme === "dark" ? styles.themeButtonActive : styles.themeButton}
        onClick={() => setTheme("dark")}
        aria-label="Switch to dark theme"
      >
        <svg viewBox="0 0 24 24" aria-hidden="true">
          <path
            d="M21 14.1A8.6 8.6 0 0 1 10 3a9.7 9.7 0 1 0 11 11.1Z"
            fill="none"
            stroke="currentColor"
            strokeLinecap="round"
            strokeLinejoin="round"
            strokeWidth="1.5"
          />
        </svg>
      </button>
    </div>
  );
}

export default function Home() {
  const [theme, setTheme] = useState<Theme>("light");
  const [copiedCommand, setCopiedCommand] = useState<string | null>(null);

  useEffect(() => {
    const root = document.documentElement;
    const currentTheme = root.dataset.theme === "dark" ? "dark" : "light";
    setTheme(currentTheme);
  }, []);

  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    window.localStorage.setItem("expect-clone-theme", theme);
  }, [theme]);

  useEffect(() => {
    if (!copiedCommand) {
      return;
    }

    const timeout = window.setTimeout(() => {
      setCopiedCommand(null);
    }, 1400);

    return () => window.clearTimeout(timeout);
  }, [copiedCommand]);

  async function handleCopy(command: string) {
    try {
      await navigator.clipboard.writeText(command);
      setCopiedCommand(command);
    } catch {
      setCopiedCommand(null);
    }
  }

  return (
    <main className={styles.page}>
      <div className={styles.hero}>
        <section className={styles.mockupWrap} aria-label="Expect browser demo">
          <div className={styles.glow} aria-hidden="true" />

          <div className={styles.browserFrame}>
            <div className={styles.browserChrome} aria-hidden="true">
              <span />
              <span />
              <span />
            </div>

            <div className={styles.formStage}>
              <p className={styles.formLabel}>Sign up</p>
              <div className={styles.formFields}>
                <div className={styles.formInput} />
                <div className={styles.formInput} />
              </div>
              <div className={styles.formButton}>
                <span />
              </div>
            </div>
          </div>

          <div className={styles.cliCard}>
            <div className={styles.browserChrome} aria-hidden="true">
              <span />
              <span />
              <span />
            </div>

            <div className={styles.cliBody}>
              <div className={styles.cliPrompt}>
                <span>$</span>
                <strong>expect</strong>
              </div>

              <div className={styles.taskList}>
                <div className={styles.taskRow}>
                  <span className={`${styles.spinner} ${styles.spinnerActive}`} aria-hidden="true" />
                  <span>Fill form</span>
                </div>
                <div className={styles.taskRow}>
                  <span className={styles.taskDot} aria-hidden="true" />
                  <span>Submit form</span>
                </div>
                <div className={styles.taskRow}>
                  <span className={styles.taskDot} aria-hidden="true" />
                  <span>Redirect page</span>
                </div>
              </div>

              <p className={styles.cliLabel}>Expect CLI</p>
            </div>
          </div>
        </section>

        <section className={styles.copy}>
          <h1 className={styles.title}>Let agents test your code in a real browser</h1>
          <p className={styles.description}>
            One command scans your unstaged changes or branch diff, then generates a
            test plan, and runs it against a live browser.
          </p>
          <a className={styles.demoButton} href="/replay?demo=true">
            View demo
          </a>
        </section>

        <section className={styles.install}>
          {installCommands.map((item) => (
            <div key={item.command} className={styles.installGroup}>
              <h2 className={styles.installHeading}>{item.label}</h2>
              <div className={styles.commandCard}>
                <div className={styles.commandText}>
                  <span className={styles.commandPrompt}>$</span>
                  <code>{item.command}</code>
                </div>
                <CopyButton
                  command={item.command}
                  copied={copiedCommand === item.command}
                  onCopy={handleCopy}
                />
              </div>
            </div>
          ))}
        </section>
      </div>

      <footer className={styles.footer}>
        <div className={styles.footerLinks}>
          {links.map((link) => (
            <a key={link.label} href={link.href} target="_blank" rel="noreferrer">
              {link.label}
            </a>
          ))}
        </div>
        <ThemeToggle theme={theme} setTheme={setTheme} />
      </footer>
    </main>
  );
}
