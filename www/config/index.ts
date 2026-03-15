import site from "@/config/site.json";
import marketing from "@/config/marketing.json";
import products from "@/config/products.json";
import dashboard from "@/config/dashboard.json";
import auth from "@/config/auth.json";
import flags from "@/config/flags.json";
import copy from "@/config/copy.json";
import type {
  SiteConfig,
  MarketingConfig,
  ProductsConfig,
  DashboardConfig,
  AuthConfig,
  FlagsConfig,
  CopyConfig,
} from "@/types/config";

export const siteConfig = site as SiteConfig;
export const marketingConfig = marketing as MarketingConfig;
export const productsConfig = products as ProductsConfig;
export const dashboardConfig = dashboard as DashboardConfig;
export const authConfig = auth as AuthConfig;
export const flagsConfig = flags as FlagsConfig;
export const copyConfig = copy as CopyConfig;
