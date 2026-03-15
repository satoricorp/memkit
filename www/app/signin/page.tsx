"use client";

import { useAuthActions } from "@convex-dev/auth/react";
import { useState } from "react";
import Link from "next/link";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import OAuthButton from "@/components/oauth-button";
import { authConfig } from "@/config";

export default function SignInPage() {
  const { signIn } = useAuthActions();
  const [isLoading, setIsLoading] = useState(false);

  const handleOAuth = async (provider: "github" | "google") => {
    if (isLoading) return;
    setIsLoading(true);
    try {
      const result = await signIn(provider, { redirectTo: authConfig.redirectTo });
      if (result?.redirect) {
        window.location.assign(result.redirect.toString());
      }
    } finally {
      setIsLoading(false);
    }
  };

  return (
    <div className="min-h-screen flex items-center justify-center p-6">
      <Card className="w-full max-w-md border-border/70 bg-background/70">
        <CardHeader>
          <CardTitle>{authConfig.signIn.title}</CardTitle>
          <CardDescription>{authConfig.signIn.subtitle}</CardDescription>
        </CardHeader>
        <CardContent>
          <div className="space-y-4">
            {authConfig.providers.map((provider) => (
              <OAuthButton
                key={provider.id}
                provider={provider.id}
                label={provider.label}
                onClick={() => handleOAuth(provider.id)}
                isLoading={isLoading}
              />
            ))}
            <div className="mt-4 text-center">
              <p className="text-sm text-muted-foreground">
                {authConfig.signIn.footer} {" "}
                <Link
                  href="/signup"
                  className="text-foreground underline underline-offset-4"
                >
                  Sign up
                </Link>
              </p>
            </div>
          </div>
        </CardContent>
      </Card>
    </div>
  );
}
