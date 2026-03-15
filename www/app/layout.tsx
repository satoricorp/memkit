import type { Metadata } from "next";
import ConvexAuthProvider from "@/components/convex-auth-provider";
import { ConvexAuthNextjsServerProvider } from "@convex-dev/auth/nextjs/server";
import { ThemeProvider } from "@/components/theme-provider";
import { Geist, Geist_Mono } from "next/font/google";
import { GeistPixelSquare } from "geist/font/pixel";
import localFont from "next/font/local";
import { siteConfig } from "@/config";
import "./globals.css";

const geistSans = Geist({
  variable: "--font-geist-sans",
  subsets: ["latin"],
});

const geistMono = Geist_Mono({
  variable: "--font-geist-mono",
  subsets: ["latin"],
});

const geistPixel = GeistPixelSquare;

const flauta = localFont({
  src: "../public/fonts/flauta.ttf",
  variable: "--font-flauta",
  display: "swap",
});

const appleGaramond = localFont({
  src: [
    {
      path: "../public/fonts/AppleGaramond.ttf",
      style: "normal",
    },
    {
      path: "../public/fonts/AppleGaramond-Italic.ttf",
      style: "italic",
    },
  ],
  variable: "--font-apple-garamond",
  display: "swap",
});

const fkGroteskBlack = localFont({
  src: "../public/fonts/FKGrotesk-Black.otf",
  variable: "--font-fk-grotesk-black",
  display: "swap",
});

export const metadata: Metadata = {
  title: siteConfig.meta.title,
  description: siteConfig.meta.description,
};

export default function RootLayout({
  children,
}: Readonly<{
  children: React.ReactNode;
}>) {
  return (
    <html lang="en" suppressHydrationWarning>
      <body
        className={`${geistSans.variable} ${geistMono.variable} ${geistPixel.variable} ${flauta.variable} ${appleGaramond.variable} ${fkGroteskBlack.variable} antialiased`}
      >
        <ConvexAuthNextjsServerProvider>
          <ConvexAuthProvider>
            <ThemeProvider
              attribute="class"
              defaultTheme="light"
              enableSystem={true}
              disableTransitionOnChange
            >
              {children}
            </ThemeProvider>
          </ConvexAuthProvider>
        </ConvexAuthNextjsServerProvider>
      </body>
    </html>
  );
}
