Create a Rust cli utility to managing launching Docker sandboxes.

Key features:
- makes it easy to create a sandbox using a common configuration that will evolve over time. Will not exist initially, but expect that we will be creating a common configuration including egress domains, secrets, and agent specific configurations (such as defining hooks, guardrails, agents.md files, etc)
- allow easy model comparison for the same task. Define a prompt/task and two to four models and generate a sandbox for each model, passing in the prompt/task, and eval/success criteria. We will then compare the responses and: 1) have a human evaluate each response, 2) use cosine comparison to see how close the responses are to one another, 3) use an LLM judge to determine which response wins.

A base directory will be specified.
Then, to create a docker sandbox, will define a project name. If a folder in the base directory with the project name already exists, use the existing directory.
If there is not directory with the project name, create a directory, then create a sandbox using the new directory.
