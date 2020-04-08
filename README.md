# Continuous deployment service for ECR and Docker Swarm

swarm-ecr-deployer is a simple service to achieve continuous deployment of ECR-hosted images onto Docker Swarm. You feed it Cloudwatch Events from ECR repositories, and swarm-ecr-deployer will examine local services and force an image update on any service with a matching image. For example, you may push to 12346689012.dkr.ecr.eu-east-1.amazonaws.com/some-service:latest. Any local service which lists that image (including the tag) will be updated.

This project is available as a Docker image via https://hub.docker.com/r/bittrance/swarm-ecr-deployer.

## Getting started

Load the example CloudFormation stack in the region where your repositories are located.

```bash
aws --region eu-central-1 cloudformation create-stack --stack-name swarm-ecr-deployer --template-body file://ecr-events.yml --capabilities CAPABILITY_NAMED_IAM
```

Now that the `swarm-ecr-deployer` user exists, you can request an access token for it:

```bash
aws iam create-access-key --user-name swarm-ecr-deployer
```

This gives you an access key and a secret. Edit the [credentials file](./swarm/aws_credentials) and load the provided Docker Swarm stack in your swarm. It will set up two replicas of the deployer to be sure it is resilient and responsive.

```bash
docker stack deploy -c swarm/swarm-ecr-deployer.yml swarm-ecr-deployer
```

To test the deployer, you can push your favorite ECR repository and watch the deployer logs.

```
docker service logs -f swarm-ecr-deployer_deployer
```
When it triggers an update, you will see
```
2020-04-08T17:18:09+02:00 - INFO - Updated service e2wjw1e4xmb1ur65h27imu3lw with image 12346689012.dkr.ecr.eu-east-1.amazonaws.com/some-service:latest, sha256:5e7cd99f4b1e1fb27607f37228a10205e4d4ae5938bfd1db2f6ca7442dd955e9
```

Check the creation time with `docker service ps <your-service>` to see that your service has been reloaded.

**Note that this setup only works for a single swarm. Each swarm needs its own queue.**

## Production setup

Since you can run multiple replicas of the deployer, there should be no practical limit to the amount of updates your swarm can receive.

A CloudWatch Events rule can only have five targets, so if you have many swarms, you need to insert an SNS topic in your setup to fan out into queues.

The deployer uses long-polling, so each deployer instance costs about USD 0.05/month in API calls.

## Limitations

In its current form, the deployer has some limitations:

- it makes no attempt to protect against out-of-order updates, nor does it throttles updates or take any special precautions on startup. If this becomes an issue, you may want to set a low `MessageRetentionPeriod` on the queue and/or switch to using a SQS FIFO queue.
- it does not track the progress or result of an update, nor does it debounce or delay updates when multiple ECR events arrive in a short time frame. It may thus trigger an update for a service that is already updating or trigger updates on many services in parallel.

## Develop

swarm-ecr-deployer uses stable rust and cargo.

```bash
rustup install stable
cargo build --release
```

Sadly, there are no tests yet.
