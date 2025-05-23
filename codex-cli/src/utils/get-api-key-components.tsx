import { providers } from "./providers.js";
import SelectInput from "../components/select-input/select-input.js";
import Spinner from "../components/vendor/ink-spinner.js";
import TextInput from "../components/vendor/ink-text-input.js";
import { Box, Text } from "ink";
import React, { useState } from "react";

export type Choice = { type: "signin" } | { type: "apikey"; key: string };

export function ApiKeyPrompt({
  onDone,
  provider = "openai",
}: {
  onDone: (choice: Choice) => void;
  provider?: string;
}): JSX.Element {
  const [step, setStep] = useState<"select" | "paste">("select");
  const [apiKey, setApiKey] = useState("");
  const providerInfo = providers[provider.toLowerCase()];

  if (!providerInfo) {
    throw new Error(`Unknown provider: ${provider}`);
  }

  if (step === "select") {
    const isOpenAI = provider.toLowerCase() === "openai";
    return (
      <Box flexDirection="column" gap={1}>
        <Box flexDirection="column">
          <Text>
            {isOpenAI
              ? "Sign in with ChatGPT to generate an API key or paste one you already have."
              : `Please provide your ${providerInfo.name} API key.`}
          </Text>
          <Text dimColor>[use arrows to move, enter to select]</Text>
        </Box>
        <SelectInput
          items={
            isOpenAI
              ? [
                  { label: "Sign in with ChatGPT", value: "signin" },
                  {
                    label: `Paste an API key (or set as ${providerInfo.envKey})`,
                    value: "paste",
                  },
                ]
              : [
                  {
                    label: `Paste an API key (or set as ${providerInfo.envKey})`,
                    value: "paste",
                  },
                ]
          }
          onSelect={(item: { value: string }) => {
            if (item.value === "signin") {
              onDone({ type: "signin" });
            } else {
              setStep("paste");
            }
          }}
        />
      </Box>
    );
  }

  return (
    <Box flexDirection="column">
      <Text>
        Paste your {providerInfo.name} API key and press &lt;Enter&gt;:
      </Text>
      <TextInput
        value={apiKey}
        onChange={setApiKey}
        onSubmit={(value: string) => {
          if (value.trim() !== "") {
            onDone({ type: "apikey", key: value.trim() });
          }
        }}
        placeholder="sk-..."
        mask="*"
      />
    </Box>
  );
}

export function WaitingForAuth(): JSX.Element {
  return (
    <Box flexDirection="row" marginTop={1}>
      <Spinner type="ball" />
      <Text>
        {" "}
        Waiting for authentication… <Text dimColor>ctrl + c to quit</Text>
      </Text>
    </Box>
  );
}
