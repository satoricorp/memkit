import { Checkout } from "@polar-sh/nextjs";
import { NextRequest } from "next/server";

export const GET = (request: NextRequest) => {
  const searchParams = request.nextUrl.searchParams;
  const orgName = searchParams.get('org_name');
  const appUrl = process.env.NEXT_PUBLIC_APP_URL ?? 'http://localhost:3000';
  const successUrl =
    (process.env.SUCCESS_URL ?? appUrl) +
    `?checkout=success${orgName ? `&org_name=${encodeURIComponent(orgName)}` : ''}`;

  return Checkout({
    accessToken: process.env.NODE_ENV === 'development'
      ? process.env.POLAR_SANDBOX_ACCESS_TOKEN ?? ''
      : process.env.POLAR_ACCESS_TOKEN ?? '',
    successUrl,
    returnUrl: appUrl,
    server: process.env.NODE_ENV === 'development' ? 'sandbox' : 'production',
    theme: 'dark',
  })(request);
};

