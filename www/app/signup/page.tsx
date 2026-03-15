"use client";

import { useAuthActions } from "@convex-dev/auth/react";
import { useState } from "react";
import Link from "next/link";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Separator } from "@/components/ui/separator";
import OAuthButton from "@/components/oauth-button";
import { authConfig } from "@/config";

export default function SignUpPage() {
  const { signIn } = useAuthActions();
  const [isSubmitting, setIsSubmitting] = useState(false);

  const handleOAuth = async (provider: "github" | "google") => {
    if (isSubmitting) return;
    setIsSubmitting(true);
    try {
      const result = await signIn(provider, { redirectTo: authConfig.redirectTo });
      if (result?.redirect) {
        window.location.assign(result.redirect.toString());
      }
    } finally {
      setIsSubmitting(false);
    }
  };

  return (
    <div className="min-h-screen flex items-center justify-center p-6">
      <Card className="w-full max-w-md border-border/70 bg-background/70">
        <CardHeader>
          <CardTitle>{authConfig.signUp.title}</CardTitle>
          <CardDescription>{authConfig.signUp.subtitle}</CardDescription>
        </CardHeader>
        <CardContent>
          <div className="space-y-4">
            <div className="relative">
              <div className="absolute inset-0 flex items-center">
                <Separator className="w-full" />
              </div>
              <div className="relative flex justify-center text-xs uppercase">
                <span className="bg-background px-2 text-muted-foreground">
                  Choose a sign up method
                </span>
              </div>
            </div>
            {authConfig.providers.map((provider) => (
              <OAuthButton
                key={provider.id}
                provider={provider.id}
                label={provider.label}
                onClick={() => handleOAuth(provider.id)}
                isLoading={isSubmitting}
              />
            ))}
          </div>
          <div className="mt-4 text-center">
            <p className="text-sm text-muted-foreground">
              {authConfig.signUp.footer} {" "}
              <Link
                href="/signin"
                className="text-foreground underline underline-offset-4"
              >
                Sign in
              </Link>
            </p>
          </div>
        </CardContent>
      </Card>
    </div>
  );
}
