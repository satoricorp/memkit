"use client";

import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { useState } from "react";
import { cn } from "@/lib/utils";
import { postEmail } from "@/app/actions/resend";

interface SignupModalProps {
  className?: string;
  children?: React.ReactNode;
}

export default function SignupModal({
  className,
  children
}: SignupModalProps) {
  const [email, setEmail] = useState("");
  const [isSubmitting, setIsSubmitting] = useState(false);
  const [isOpen, setIsOpen] = useState(false);
  const [message, setMessage] = useState("");
  const [isSuccess, setIsSuccess] = useState(false);

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    setIsSubmitting(true);
    setMessage("");
    setIsSuccess(false);

    const formData = new FormData();
    formData.append("email", email);

    try {
      const result = await postEmail(formData);
      if (result) {
        if (result.includes("release more seats")) {
          setIsSuccess(true);
          setMessage("Thanks for signing up!🔥 You're on the list!");
          setTimeout(() => {
            setEmail("");
            setIsOpen(false);
            setMessage("");
          }, 4000);
        } else {
          setMessage(result);
        }
      }
    } catch (error) {
      console.error("Error submitting email:", error);
      setMessage("An error occurred. Please try again.");
    } finally {
      setIsSubmitting(false);
    }
  };

  const triggerContent = children || "Get Updates";

  return (
    <Dialog open={isOpen} onOpenChange={(open) => {
      setIsOpen(open);
      if (!open) {
        setMessage("");
        setIsSuccess(false);
      }
    }}>
      <DialogTrigger asChild>
        <button
          className={cn(
            "min-w-[180px] px-4 py-2 font-mono font-medium bg-black text-white rounded hover:bg-gray-800 transition-colors dark:bg-white dark:text-black dark:hover:bg-gray-200 group flex items-center gap-2",
            className
          )}
        >
          {triggerContent}
        </button>
      </DialogTrigger>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>Get Updates</DialogTitle>
          <DialogDescription>
            We&apos;ll send you updates when we ship.
          </DialogDescription>
        </DialogHeader>
        <form onSubmit={handleSubmit} className="space-y-4">
          <div className="space-y-2">
            <Input
              type="email"
              placeholder="Enter your email address"
              value={email}
              onChange={(e) => setEmail(e.target.value)}
              required
              className="w-full"
              disabled={isSuccess}
            />
          </div>
          {message && (
            <p className={cn(
              "text-sm",
              isSuccess ? "text-green-600 dark:text-green-400" : "text-red-600 dark:text-red-400"
            )}>
              {message}
            </p>
          )}
          <Button
            type="submit"
            disabled={isSubmitting || isSuccess}
            className="w-full"
          >
            {isSubmitting ? "Joining..." : isSuccess ? "Success!" : "Get Updates"}
          </Button>
        </form>
      </DialogContent>
    </Dialog>
  );
}

