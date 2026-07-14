/**
 * Offline local stand-in for the Base44 SDK client.
 * Same export shape (`base44.entities.*`, `base44.auth.*`) so call sites keep working
 * without network or hosted-platform env.
 */
import { AgentTemplate } from "./localAgentTemplateStore";

const LOCAL_USER = {
  id: "local-user",
  email: "local@offline",
  full_name: "Local User",
  role: "admin",
};

const auth = {
  async me() {
    return LOCAL_USER;
  },
  async logout(_redirectUrl) {
    // no-op offline
  },
  redirectToLogin(_returnUrl) {
    // no-op offline — app boots without auth
  },
  async loginViaEmailPassword(_email, _password) {
    return { access_token: "local", user: LOCAL_USER };
  },
  loginWithProvider(_provider, _returnUrl) {
    // no-op offline
  },
  async register({ email }) {
    return { email, status: "registered" };
  },
  async verifyOtp({ email }) {
    return { access_token: "local", email };
  },
  setToken(_token) {
    // no-op offline
  },
  async resendOtp(_email) {
    // no-op offline
  },
  async resetPasswordRequest(_email) {
    // no-op offline
  },
  async resetPassword(_opts) {
    // no-op offline
  },
};

export const base44 = {
  entities: {
    AgentTemplate,
  },
  auth,
};
