FROM mcr.microsoft.com/dotnet/sdk:10.0 AS build
WORKDIR /src
COPY KlarkHub.csproj .
RUN dotnet restore
COPY . .
RUN dotnet publish -c Release -o /app

FROM mcr.microsoft.com/dotnet/aspnet:10.0
WORKDIR /app
COPY --from=build /app .
ENV ASPNETCORE_URLS=http://+:7470 \
    Hub__DataDir=/data
# hub.db, machines/*.db (each capped at 10 MB) and assets/ (temporary, swept after
# Hub:Assets:RetentionHours, default 24) — keep this on a volume. One process, one port.
VOLUME /data
EXPOSE 7470
ENTRYPOINT ["dotnet", "KlarkHub.dll"]
