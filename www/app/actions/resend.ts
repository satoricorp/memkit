'use server'
import { Resend } from 'resend';
import { siteConfig } from '@/config';

const resend = new Resend(process.env.RESEND_API_KEY!);

export async function postEmail(formData: FormData) {
  const email = formData.get('email')?.toString();

  if (!email) {
    return 'Email is required';
  }

  try {
    const res = await resend.contacts.create({
      email,
      unsubscribed: false,
      audienceId: process.env.RESEND_AUDIENCE_ID!,
    });

    if (res.error) {
      // Check for specific error types
      if (res.error.name === 'validation_error' && res.error.message === 'API key is invalid') {
        return 'Configuration error. Please contact support.';
      }
      return 'Failed to add to email list. Please try again.';
    }

    if (res.data) {
      return `Thanks for signing up!🔥 You're on the list!`;
    }

    return 'Something went wrong. Please try again.';
  } catch (error) {
    return 'Something went wrong. Please try again.';
  }
}

export async function sendWelcomeEmail(email: string, orgName: string) {
  try {
    const brandName = siteConfig.brand.name;
    const fromAddress =
      process.env.WELCOME_FROM ?? `${brandName} <welcome@yourcompany.com>`;
    const { data: _data, error } = await resend.emails.send({
      from: fromAddress,
      to: [email],
      subject: `Welcome to ${brandName}, ${orgName}!`,
      html: `
        <div style="font-family: Arial, sans-serif; max-width: 600px; margin: 0 auto;">
          <h1 style="color: #333;">Welcome to ${brandName}!</h1>
          <p>Hi there,</p>
          <p>Thank you for signing up for ${brandName}. Your organization <strong>${orgName}</strong> has been created successfully.</p>
          <p>You can now start building with our platform. Your API key has been generated and is ready to use.</p>
          <p>If you have any questions, feel free to reach out to our support team.</p>
          <p>Happy building!</p>
          <p>The ${brandName} Team</p>
        </div>
      `,
    });

    if (error) {
      console.error('Failed to send welcome email:', error);
      return { success: false, error: error.message };
    }

    return { success: true };
  } catch (error) {
    console.error('Error sending welcome email:', error);
    return { success: false, error: String(error) };
  }
}
