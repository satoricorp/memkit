"use client";

import { ConvexAuthNextjsProvider } from "@convex-dev/auth/nextjs";
import { ConvexReactClient } from "convex/react";
import { ReactNode, useMemo } from "react";

type Props = {
  children: ReactNode;
};

export default function ConvexAuthProviderWrapper({ children }: Props) {
  const convexUrl = process.env.NEXT_PUBLIC_CONVEX_URL ?? "";
  const client = useMemo(() => new ConvexReactClient(convexUrl), [convexUrl]);
  return (
    <ConvexAuthNextjsProvider client={client}>
      {children}
    </ConvexAuthNextjsProvider>
  );
}
