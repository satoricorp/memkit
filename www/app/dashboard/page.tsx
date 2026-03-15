"use client";

import { useEffect } from "react";
import { useRouter } from "next/navigation";
import { useAuthToken } from "@convex-dev/auth/react";
import Nav from "@/components/nav";
import Footer from "@/components/footer";
import Dashboard from "@/components/dashboard";

export default function DashboardPage() {
  const token = useAuthToken();
  const router = useRouter();

  useEffect(() => {
    if (token === null) {
      router.push("/signin");
    }
  }, [router, token]);

  if (!token) {
    return null;
  }

  return (
    <>
      <Nav />
      <Dashboard />
      <Footer />
    </>
  );
}
