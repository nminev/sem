# sem-diff-lambda

AWS Lambda that runs [sem](https://github.com/Ataraxy-Labs/sem) semantic diff on two JSON payloads. Instead of a line-by-line diff, it returns a structured list of which keys were added, removed, modified, or renamed.

## What it does

Send it two JSON objects and it tells you what semantically changed between them.

## Input

```json
{
  "original": { "version": "1.0.0", "timeout": 30 },
  "modified": { "version": "2.0.0", "request_timeout": 30 }
}
```

Optional fields:
- `filename` — name to use for the file in the output (default: `data.json`)
- `format` — `json` or `terminal` (default: `json`)

## Output

```json
{
  "statusCode": 200,
  "body": {
    "changes": [
      {
        "entityName": "version",
        "entityType": "property",
        "changeType": "modified",
        "beforeContent": "\"version\": \"1.0.0\"",
        "afterContent": "\"version\": \"2.0.0\""
      },
      {
        "entityName": "request_timeout",
        "entityType": "property",
        "changeType": "renamed"
      }
    ],
    "summary": { "added": 0, "modified": 1, "deleted": 0, "renamed": 1 }
  }
}
```

## Deploying

Requires Docker, AWS CLI, and an ECR repository. Run from the **repo root**, not from `lambda/` — the Dockerfile pulls in `crates/` from the build context.

```bash
# Build (from repo root)
docker build --platform linux/arm64 -f lambda/Dockerfile -t sem-lambda .

# Push to ECR
aws ecr get-login-password --region <region> | docker login --username AWS --password-stdin <account>.dkr.ecr.<region>.amazonaws.com
docker tag sem-lambda:latest <account>.dkr.ecr.<region>.amazonaws.com/sem-lambda:latest
docker push <account>.dkr.ecr.<region>.amazonaws.com/sem-lambda:latest

# Create function
aws lambda create-function \
  --function-name sem-diff \
  --package-type Image \
  --code ImageUri=<account>.dkr.ecr.<region>.amazonaws.com/sem-lambda:latest \
  --role <execution-role-arn> \
  --architectures arm64 \
  --timeout 30 \
  --memory-size 512

# Or update an existing function
aws lambda update-function-code \
  --function-name sem-diff \
  --image-uri <account>.dkr.ecr.<region>.amazonaws.com/sem-lambda:latest
```

## How it works

Multi-stage Docker build: stage 1 compiles `sem-cli` from this repo's `crates/` directory inside Amazon Linux 2023, stage 2 copies the binary into the Lambda Python runtime. No git repo needed at runtime — `sem diff --stdin` accepts file changes directly as JSON.
