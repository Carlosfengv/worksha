export interface CostTotals {
  actual: { currency: string; amount: number; calls: number }[];
  estimated: { currency: string; amount: number; calls: number }[];
  unknownCalls: number;
}
export interface AssociatedCosts {
  cost: CostTotals;
  gatewayAttempts: number;
  responseLinkedAttempts: number;
  sessionLinkedAttempts: number;
  nativeResponses: number;
  matchedNativeResponses: number;
  unlinkedNativeResponses: number;
  conflictingNativeResponses: number;
  sourceState: string;
}
