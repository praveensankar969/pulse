import type {
  AppSettings,
  AssertOp,
  CheckEvidence,
  CheckResult,
  ExpectedStatus,
  HttpMethod,
  Service,
  Theme,
} from "./types";
import checkEvidence from "../../schema/fixtures/check-evidence.json";
import checkResult from "../../schema/fixtures/check-result.json";
import service from "../../schema/fixtures/service.json";
import settings from "../../schema/fixtures/settings.json";

function expectedStatus(value: string | number | number[]): ExpectedStatus {
  return value as ExpectedStatus;
}

function method(value: string): HttpMethod {
  return value as HttpMethod;
}

function op(value: string): AssertOp {
  return value as AssertOp;
}

/** Same JSON fixtures `cargo test types_match` round-trips through serde. */
export const sampleService: Service = {
  ...service,
  method: method(service.method),
  expectedStatus: expectedStatus(service.expectedStatus),
  assertions: service.assertions.map((assertion) => ({
    ...assertion,
    op: op(assertion.op),
  })),
};

export const sampleSettings: AppSettings = {
  ...settings,
  theme: settings.theme as Theme,
};

export const sampleCheckEvidence: CheckEvidence = {
  ...checkEvidence,
  outcome: checkEvidence.outcome as CheckEvidence["outcome"],
  assertionResults: checkEvidence.assertionResults.map((result) => ({
    ...result,
    op: op(result.op),
  })),
};

export const sampleCheckResult: CheckResult = {
  ...sampleCheckEvidence,
  state: checkResult.state as CheckResult["state"],
};
