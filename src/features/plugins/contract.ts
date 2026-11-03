import manifestSchema from "../../../contracts/plugins/manifest.schema.json";
import reviewSchema from "../../../contracts/plugins/package-review.schema.json";
import statusSchema from "../../../contracts/plugins/status.schema.json";
import viewSchema from "../../../contracts/plugins/view.schema.json";
import sessionSchema from "../../../contracts/plugins/session-candidate.schema.json";
import quotaSchema from "../../../contracts/plugins/quota.schema.json";
import conversationSchema from "../../../contracts/plugins/conversation.schema.json";
import reportSchema from "../../../contracts/plugins/gateway-report.schema.json";

import { acceptsSchema } from "@/shared/lib/contract-schema";
export { object, parseSchema } from "@/shared/lib/contract-schema";
const schemas = {
  report: reportSchema,
  manifest: manifestSchema,
  review: reviewSchema,
  status: statusSchema,
  view: viewSchema,
  session: sessionSchema,
  quota: quotaSchema,
  conversation: conversationSchema,
};
export function parseContract<T>(
  name: keyof typeof schemas,
  value: unknown,
): T {
  if (!acceptsSchema(schemas[name], value))
    throw new Error("插件响应不符合已登记接口，请重新读取。");
  return value as T;
}
